//! Shared encoded media ownership across producers, Delivery, and transports.
//! A reservation precedes application body copying. Bytes owns the reservation
//! so clones, slices and HTTP response bodies cannot release it prematurely.
use anyhow::{Result, ensure};
use axum::body::Bytes;
use serde::{Serialize, Serializer};
use std::{
    collections::HashMap,
    ops::{Deref, RangeBounds},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

pub const MAX_ENCODED_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_ENCODED_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GLOBAL_ENCODED_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_PCM_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_BUDGET_OWNERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioBudgetOwner {
    Response(Uuid),
    /// Preview APIs have a request budget, not a fabricated canonical Response.
    StandaloneRequest(Uuid),
}
#[derive(Debug)]
struct Pool {
    global: Arc<Semaphore>,
    owners: Mutex<HashMap<AudioBudgetOwner, Weak<Scope>>>,
}
#[derive(Debug)]
struct Scope {
    owner: AudioBudgetOwner,
    local: Arc<Semaphore>,
    pool: Arc<Pool>,
}
impl Drop for Scope {
    fn drop(&mut self) {
        let mut owners = self.pool.owners.lock().unwrap_or_else(|e| e.into_inner());
        if owners
            .get(&self.owner)
            .is_some_and(|weak| std::ptr::eq(weak.as_ptr(), self))
        {
            owners.remove(&self.owner);
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResponseAudioBudget(Arc<Scope>);
impl ResponseAudioBudget {
    pub fn for_response(response_id: Uuid) -> Result<Self> {
        ensure!(
            !response_id.is_nil(),
            "encoded audio requires an original Response ID"
        );
        Self::for_owner(AudioBudgetOwner::Response(response_id))
    }
    pub fn for_standalone_request(request_id: Uuid) -> Result<Self> {
        ensure!(!request_id.is_nil(), "encoded audio requires a request ID");
        Self::for_owner(AudioBudgetOwner::StandaloneRequest(request_id))
    }
    fn for_owner(owner: AudioBudgetOwner) -> Result<Self> {
        static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
        let pool = POOL.get_or_init(|| {
            Arc::new(Pool {
                global: Arc::new(Semaphore::new(MAX_GLOBAL_ENCODED_BYTES)),
                owners: Mutex::new(HashMap::new()),
            })
        });
        Self::in_pool(pool.clone(), owner)
    }
    fn in_pool(pool: Arc<Pool>, owner: AudioBudgetOwner) -> Result<Self> {
        let mut owners = pool.owners.lock().unwrap_or_else(|e| e.into_inner());
        owners.retain(|_, value| value.strong_count() > 0);
        if let Some(scope) = owners.get(&owner).and_then(Weak::upgrade) {
            return Ok(Self(scope));
        }
        ensure!(
            owners.len() < MAX_BUDGET_OWNERS,
            "too many active encoded audio owners"
        );
        let scope = Arc::new(Scope {
            owner,
            local: Arc::new(Semaphore::new(MAX_RESPONSE_ENCODED_BYTES)),
            pool: pool.clone(),
        });
        owners.insert(owner, Arc::downgrade(&scope));
        Ok(Self(scope))
    }
    pub fn owner(&self) -> AudioBudgetOwner {
        self.0.owner
    }
    /// Includes in-flight copy reservations, not only completed media.
    pub fn reserved_bytes(&self) -> usize {
        MAX_RESPONSE_ENCODED_BYTES - self.0.local.available_permits()
    }
    pub async fn reserve(&self, bytes: usize) -> Result<AudioReservation> {
        ensure!(
            bytes <= MAX_ENCODED_PAYLOAD_BYTES,
            "encoded payload exceeds 1 MiB"
        );
        let local = self
            .0
            .local
            .clone()
            .acquire_many_owned(bytes as u32)
            .await?;
        let global = self
            .0
            .pool
            .global
            .clone()
            .acquire_many_owned(bytes as u32)
            .await?;
        Ok(AudioReservation {
            budget: self.clone(),
            local,
            global,
        })
    }
    pub async fn copy_from_slice(&self, bytes: &[u8]) -> Result<EncodedAudio> {
        let reservation = self.reserve(bytes.len()).await?;
        reservation.seal(bytes.to_vec())
    }
}
#[derive(Debug)]
pub struct AudioReservation {
    budget: ResponseAudioBudget,
    local: OwnedSemaphorePermit,
    global: OwnedSemaphorePermit,
}
impl AudioReservation {
    pub fn capacity(&self) -> usize {
        self.local.num_permits()
    }
    pub(crate) fn seal(mut self, bytes: Vec<u8>) -> Result<EncodedAudio> {
        // Account for retained Vec spare capacity as well as initialized bytes.
        ensure!(
            bytes.capacity() <= self.capacity(),
            "audio copy exceeded its prior reservation"
        );
        let unused = self.capacity() - bytes.capacity();
        drop(self.local.split(unused));
        drop(self.global.split(unused));
        let owner = self.budget.owner();
        Ok(EncodedAudio(AudioBuffer {
            bytes: Bytes::from_owner(ReservedBytes {
                bytes,
                _reservation: self,
            }),
            owner: Some(owner),
        }))
    }
}
struct ReservedBytes {
    bytes: Vec<u8>,
    _reservation: AudioReservation,
}
impl AsRef<[u8]> for ReservedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Only a prior encoded reservation can construct a provider's output.
#[derive(Clone, Debug)]
pub struct EncodedAudio(AudioBuffer);
impl EncodedAudio {
    pub fn into_buffer(self) -> AudioBuffer {
        self.0
    }
    pub fn into_bytes(self) -> Bytes {
        self.0.into_bytes()
    }
    pub fn owner(&self) -> AudioBudgetOwner {
        self.0.owner.expect("encoded owner")
    }
}
impl Deref for EncodedAudio {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}
impl AsRef<[u8]> for EncodedAudio {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

/// Cloning or slicing never copies audio or obtains a second reservation.
#[derive(Clone, Debug)]
pub struct AudioBuffer {
    bytes: Bytes,
    owner: Option<AudioBudgetOwner>,
}
impl AudioBuffer {
    /// Ingress and locally generated PCM use their separate admission limits.
    /// This constructor cannot produce EncodedAudio returned by TtsAdapter.
    pub fn input(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
            owner: None,
        }
    }
    pub fn owner(&self) -> Option<AudioBudgetOwner> {
        self.owner
    }
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Self {
        Self {
            bytes: self.bytes.slice(range),
            owner: self.owner,
        }
    }
    pub fn derived_pcm(&self, bytes: Vec<u8>) -> Result<Self> {
        ensure!(
            bytes.capacity() <= MAX_PCM_PAYLOAD_BYTES,
            "decoded PCM payload exceeds 2 MiB"
        );
        struct Derived {
            bytes: Vec<u8>,
            _source: AudioBuffer,
        }
        impl AsRef<[u8]> for Derived {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        Ok(Self {
            bytes: Bytes::from_owner(Derived {
                bytes,
                _source: self.clone(),
            }),
            owner: self.owner,
        })
    }
}
impl Deref for AudioBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl AsRef<[u8]> for AudioBuffer {
    fn as_ref(&self) -> &[u8] {
        self
    }
}
impl Serialize for AudioBuffer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self)
    }
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
