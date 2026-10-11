//! Target-scoped fault injection; never changes production filesystem behavior.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use anyhow::Result;

pub(crate) use super::Stage;

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub stage: Stage,
    pub path: PathBuf,
}

enum Action {
    Fail,
    Pause,
}

struct Hook {
    stage: Stage,
    action: Mutex<Option<Action>>,
    events: Mutex<Vec<Event>>,
    entered: tokio::sync::Semaphore,
    released: Mutex<bool>,
    wake: Condvar,
}

fn hooks() -> &'static Mutex<HashMap<PathBuf, Arc<Hook>>> {
    static HOOKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Hook>>>> = OnceLock::new();
    HOOKS.get_or_init(Mutex::default)
}

pub(crate) struct Guard {
    target: PathBuf,
    hook: Arc<Hook>,
}

impl Guard {
    pub(crate) fn events(&self) -> Vec<Event> {
        self.hook.events.lock().unwrap().clone()
    }

    pub(crate) async fn entered(&self) {
        self.hook.entered.acquire().await.unwrap().forget();
    }

    pub(crate) fn release(&self) {
        *self.hook.released.lock().unwrap() = true;
        self.hook.wake.notify_all();
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.release();
        let mut hooks = hooks().lock().unwrap();
        if hooks
            .get(&self.target)
            .is_some_and(|hook| Arc::ptr_eq(hook, &self.hook))
        {
            hooks.remove(&self.target);
        }
    }
}

pub(crate) fn fail_once(target: &Path, stage: Stage) -> Guard {
    install(target, stage, Action::Fail)
}

pub(crate) fn pause_once(target: &Path, stage: Stage) -> Guard {
    install(target, stage, Action::Pause)
}

fn install(target: &Path, stage: Stage, action: Action) -> Guard {
    let target = super::destination(target).unwrap();
    let hook = Arc::new(Hook {
        stage,
        action: Mutex::new(Some(action)),
        events: Mutex::new(Vec::new()),
        entered: tokio::sync::Semaphore::new(0),
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    let mut hooks = hooks().lock().unwrap();
    assert!(
        !hooks.contains_key(&target),
        "one publication fault guard per target"
    );
    hooks.insert(target.clone(), hook.clone());
    Guard { target, hook }
}

pub(super) fn checkpoint(target: &Path, stage: Stage, path: &Path) -> Result<()> {
    let hook = hooks().lock().unwrap().get(target).cloned();
    let Some(hook) = hook else {
        return Ok(());
    };
    hook.events.lock().unwrap().push(Event {
        stage,
        path: path.to_path_buf(),
    });
    if stage != hook.stage {
        return Ok(());
    }
    let action = hook.action.lock().unwrap().take();
    match action {
        Some(Action::Fail) => anyhow::bail!("injected configuration {stage:?} failure"),
        Some(Action::Pause) => {
            hook.entered.add_permits(1);
            let mut released = hook.released.lock().unwrap();
            while !*released {
                released = hook.wake.wait(released).unwrap();
            }
        }
        None => {}
    }
    Ok(())
}
