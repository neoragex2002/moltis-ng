use std::sync::{Mutex, MutexGuard};

struct TestDirsState;

static TEST_DIRS_LOCK: Mutex<TestDirsState> = Mutex::new(TestDirsState);

pub(crate) struct TestDirsGuard {
    _lock: MutexGuard<'static, TestDirsState>,
    _tmp: Option<tempfile::TempDir>,
    overrides: bool,
}

impl TestDirsGuard {
    pub(crate) fn new() -> Self {
        let lock = TEST_DIRS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        moltis_config::set_config_dir(tmp.path().to_path_buf());
        moltis_config::set_data_dir(tmp.path().to_path_buf());
        Self {
            _lock: lock,
            _tmp: Some(tmp),
            overrides: true,
        }
    }

    pub(crate) fn lock_only() -> Self {
        let lock = TEST_DIRS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Self {
            _lock: lock,
            _tmp: None,
            overrides: false,
        }
    }
}

impl Drop for TestDirsGuard {
    fn drop(&mut self) {
        if self.overrides {
            moltis_config::clear_config_dir();
            moltis_config::clear_data_dir();
        }
    }
}
