use super::*;
use futures::poll;
use std::task::Poll;
use std::time::Duration;

fn pool() -> Arc<Pool> {
    Arc::new(Pool {
        global: Arc::new(Semaphore::new(MAX_GLOBAL_ENCODED_BYTES)),
        owners: Mutex::new(HashMap::new()),
    })
}
fn scope(pool: &Arc<Pool>) -> ResponseAudioBudget {
    ResponseAudioBudget::in_pool(pool.clone(), AudioBudgetOwner::Response(Uuid::new_v4())).unwrap()
}

#[tokio::test]
async fn same_response_cannot_renew_or_bypass_held_capacity() {
    let pool = pool();
    let original = scope(&pool);
    let retry = ResponseAudioBudget::in_pool(pool.clone(), original.owner()).unwrap();
    assert!(Arc::ptr_eq(&original.0, &retry.0));
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(original.reserve(MAX_ENCODED_PAYLOAD_BYTES).await.unwrap());
    }
    let mut pending = Box::pin(retry.reserve(1));
    assert!(matches!(poll!(pending.as_mut()), Poll::Pending));
    assert_eq!(original.reserved_bytes(), MAX_RESPONSE_ENCODED_BYTES);
    drop(held.pop());
    let permit = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permit.capacity(), 1);
    drop(permit);
    drop(held);
    assert_eq!(retry.reserved_bytes(), 0);
}

#[tokio::test]
async fn clones_slices_queue_derived_pcm_and_http_bytes_share_last_drop() {
    let budget = scope(&pool());
    let audio = budget.copy_from_slice(&[3; 96]).await.unwrap();
    let clone = audio.clone();
    assert_eq!(audio.as_ptr(), clone.as_ptr());
    assert_eq!(budget.reserved_bytes(), 96);
    let buffer = audio.into_buffer();
    let slice = buffer.slice(12..24);
    assert_eq!(slice.as_ptr(), buffer.as_ptr().wrapping_add(12));
    let pcm = slice.derived_pcm(vec![4; 32]).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    tx.send(pcm).await.unwrap();
    drop((clone, buffer, slice, tx));
    assert_eq!(budget.reserved_bytes(), 96);
    let queued = rx.recv().await.unwrap().into_bytes();
    let http_clone = queued.clone();
    drop((queued, rx));
    assert_eq!(budget.reserved_bytes(), 96);
    drop(http_clone);
    assert_eq!(budget.reserved_bytes(), 0);
}

#[tokio::test]
async fn global_capacity_is_shared_and_cancelled_waiter_releases_local_credit() {
    let pool = pool();
    let budgets = (0..32).map(|_| scope(&pool)).collect::<Vec<_>>();
    let holds = futures::future::join_all(budgets.iter().map(|budget| async move {
        let mut leases = Vec::new();
        for _ in 0..8 {
            leases.push(budget.reserve(MAX_ENCODED_PAYLOAD_BYTES).await.unwrap());
        }
        leases
    }))
    .await;
    let mut holds = holds.into_iter().flatten().collect::<Vec<_>>();
    assert_eq!(pool.global.available_permits(), 0);
    let waiting = scope(&pool);
    // The preceding 256 acquisitions may exhaust Tokio's cooperative poll
    // budget. Here we inspect semaphore queueing, not the scheduler yield.
    let mut pending = Box::pin(tokio::task::unconstrained(
        waiting.reserve(MAX_ENCODED_PAYLOAD_BYTES),
    ));
    assert!(matches!(poll!(pending.as_mut()), Poll::Pending));
    assert_eq!(waiting.reserved_bytes(), MAX_ENCODED_PAYLOAD_BYTES);
    drop(pending);
    assert_eq!(waiting.reserved_bytes(), 0);
    let first = scope(&pool);
    let second = scope(&pool);
    let mut a = Box::pin(tokio::task::unconstrained(
        first.reserve(MAX_ENCODED_PAYLOAD_BYTES),
    ));
    let mut b = Box::pin(tokio::task::unconstrained(
        second.reserve(MAX_ENCODED_PAYLOAD_BYTES),
    ));
    assert!(poll!(a.as_mut()).is_pending());
    assert!(poll!(b.as_mut()).is_pending());
    drop(holds.pop());
    let a = tokio::time::timeout(Duration::from_secs(1), a)
        .await
        .unwrap()
        .unwrap();
    assert!(poll!(b.as_mut()).is_pending());
    drop(a);
    let b = tokio::time::timeout(Duration::from_secs(1), b)
        .await
        .unwrap()
        .unwrap();
    drop((b, holds));
    assert_eq!(pool.global.available_permits(), MAX_GLOBAL_ENCODED_BYTES);
}

#[tokio::test]
async fn seal_accounts_retained_capacity_and_rejects_oversized_allocations() {
    let budget = scope(&pool());
    assert!(budget.reserve(MAX_ENCODED_PAYLOAD_BYTES + 1).await.is_err());
    let mut data = Vec::with_capacity(32);
    data.extend([1, 2, 3]);
    let encoded = budget.reserve(64).await.unwrap().seal(data).unwrap();
    assert_eq!(encoded.len(), 3);
    assert_eq!(budget.reserved_bytes(), 32);
    drop(encoded);
    assert!(budget.reserve(1).await.unwrap().seal(vec![1, 2]).is_err());
    assert_eq!(budget.reserved_bytes(), 0);
}

#[test]
fn registry_has_no_historical_uuid_growth_and_live_owners_are_bounded() {
    let pool = pool();
    for _ in 0..MAX_BUDGET_OWNERS * 2 {
        drop(scope(&pool));
    }
    assert!(pool.owners.lock().unwrap().is_empty());
    let owners = (0..MAX_BUDGET_OWNERS)
        .map(|_| scope(&pool))
        .collect::<Vec<_>>();
    assert!(
        ResponseAudioBudget::in_pool(pool.clone(), AudioBudgetOwner::Response(Uuid::new_v4()))
            .is_err()
    );
    drop(owners);
    assert!(pool.owners.lock().unwrap().is_empty());
    assert!(ResponseAudioBudget::for_response(Uuid::nil()).is_err());
    assert!(ResponseAudioBudget::for_standalone_request(Uuid::nil()).is_err());
}

#[test]
fn standalone_request_is_not_a_canonical_response_budget() {
    let id = Uuid::new_v4();
    let response = ResponseAudioBudget::for_response(id).unwrap();
    let request = ResponseAudioBudget::for_standalone_request(id).unwrap();
    assert_ne!(response.owner(), request.owner());
    assert!(!Arc::ptr_eq(&response.0, &request.0));
}
