use dc_store::{AcquireRequest, Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn concurrent_renewal_by_the_same_holder_still_returns_its_lease() {
    let dir = std::env::temp_dir().join(format!("dc-renewal-same-holder-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.set_clock(|| 1_700_000_000);
    let original = store
        .acquire(&AcquireRequest {
            task_id: "SAME".into(),
            owner: "owner".into(),
            ttl_seconds: Some(900),
            ..Default::default()
        })
        .unwrap();
    let renewed = AtomicBool::new(false);
    let rival_path = path.clone();
    let token = original.token.clone();
    store.set_clock(move || {
        if !renewed.swap(true, Ordering::SeqCst) {
            let mut rival = Store::open(&rival_path).unwrap();
            rival.set_clock(|| 1_700_000_000);
            assert!(rival.renew("SAME", &token, 1800).unwrap().is_some());
        }
        1_700_000_000
    });
    let result = store.renew("SAME", &original.token, 1800).unwrap();
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
    let result = result.expect("another renewal did not revoke this holder");
    assert_eq!(result.id, original.id);
    assert_eq!(result.token, original.token);
}

#[test]
fn renewal_never_returns_the_holder_that_replaced_its_authenticated_lease() {
    for ttl_seconds in [Some(900), None] {
        let dir = std::env::temp_dir().join(format!(
            "dc-renewal-identity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.set_clock(|| 1_700_000_000);
        let original = store
            .acquire(&AcquireRequest {
                task_id: "RACE".into(),
                owner: "original".into(),
                ttl_seconds,
                ..Default::default()
            })
            .unwrap();
        let replaced = AtomicBool::new(false);
        let rival_path = path.clone();
        // The injected clock is called after renewal loads its incumbent.
        // A real second SQLite connection replaces that lease at this exact
        // boundary, without sleeps, private SQL, or a test-only mutation hook.
        store.set_clock(move || {
            if !replaced.swap(true, Ordering::SeqCst) {
                let mut rival = Store::open(&rival_path).unwrap();
                rival.set_clock(|| 1_700_000_000);
                rival
                    .acquire(&AcquireRequest {
                        task_id: "RACE".into(),
                        owner: "replacement".into(),
                        ttl_seconds: Some(900),
                        force: true,
                        ..Default::default()
                    })
                    .unwrap();
            }
            1_700_000_000
        });
        let renewed = store.renew("RACE", &original.token, 600).unwrap();
        assert!(
            renewed.is_none(),
            "lost ownership must not return a replacement capability"
        );
        let current = store.active_lease("RACE").unwrap().unwrap();
        assert_eq!(current.owner, "replacement");
        assert_ne!(current.token, original.token);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
