use super::*;
use crate::{
    record::test_support::{Fixture, candidate},
    test_support::{Events, generation},
};
use mythos_llm::provider::{ProviderDelta, ProviderFinish};

fn trigger(now: u64, authorized: bool) -> Trigger {
    Trigger {
        authorized,
        now_seconds: now,
        idle_seconds: 60,
        blocks: 16,
        tokens: 4096,
        compression_needed: true,
    }
}
fn setup() -> (Fixture, Compression) {
    let fixture = Fixture::new();
    let session = Session::open(fixture.path.clone(), fixture.events.clone()).unwrap();
    let coordinator = Coordinator::new(Arc::new(Events::default()), 16).unwrap();
    (
        fixture,
        Compression::new(Arc::new(Mutex::new(session)), coordinator),
    )
}
#[tokio::test]
async fn disabled_unauthorized_and_failed_attempts_never_submit_recap() {
    let (_fixture, runner) = setup();
    let frozen = candidate(&mut runner.session.lock().unwrap());
    assert_eq!(
        runner
            .attempt(
                trigger(1000, true),
                frozen,
                Box::new(|_| panic!("默认关闭不得构造请求"))
            )
            .await
            .unwrap(),
        None
    );
    runner.set_enabled(true);
    let frozen = candidate(&mut runner.session.lock().unwrap());
    assert_eq!(
        runner
            .attempt(
                trigger(1000, false),
                frozen,
                Box::new(|_| panic!("未授权不得构造请求"))
            )
            .await
            .unwrap(),
        None
    );
    for now in [1000, 1300, 1600] {
        let frozen = candidate(&mut runner.session.lock().unwrap());
        assert!(
            runner
                .attempt(
                    trigger(now, true),
                    frozen,
                    Box::new(|_| Err(Fault::bad_request()))
                )
                .await
                .is_err()
        );
    }
    let frozen = candidate(&mut runner.session.lock().unwrap());
    assert_eq!(
        runner
            .attempt(
                trigger(2000, true),
                frozen,
                Box::new(|_| panic!("三次失败后停用"))
            )
            .await
            .unwrap(),
        None
    );
    runner.resume();
    let frozen = candidate(&mut runner.session.lock().unwrap());
    let seq = runner
        .attempt(
            trigger(2000, true),
            frozen,
            Box::new(|_| {
                Ok(generation(
                    vec![
                        Ok(ProviderDelta::Text("已确认摘要".into())),
                        Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
                    ],
                    false,
                )
                .0)
            }),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        runner.session.lock().unwrap().read(seq).unwrap().kind,
        "recap"
    );
}

#[tokio::test]
async fn cancel_waits_private_producer_and_releases_shared_gate_without_recap() {
    let (_fixture, runner) = setup();
    runner.set_enabled(true);
    let frozen = candidate(&mut runner.session.lock().unwrap());
    let (request, calls) = generation(vec![Ok(ProviderDelta::Text("未完成摘要".into()))], true);
    let attempt = runner.attempt(trigger(1000, true), frozen, Box::new(|_| Ok(request)));
    let cancel = async {
        while calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(runner.coordinator.acquire().is_err());
        runner.cancel();
    };
    let (result, ()) = tokio::join!(attempt, cancel);
    assert_eq!(result.unwrap(), None);
    assert!(runner.coordinator.acquire().is_ok());
    assert!(
        runner
            .session
            .lock()
            .unwrap()
            .index
            .values()
            .all(|index| index.kind != "recap")
    );
    runner.cancel();
}

#[test]
fn cancellation_is_checked_under_the_session_lock_before_commit() {
    let (_fixture, runner) = setup();
    let frozen = candidate(&mut runner.session.lock().unwrap());
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        commit(
            runner.session.clone(),
            cancel,
            frozen,
            Some("不应提交".into())
        )
        .unwrap(),
        None
    );
    assert!(
        runner
            .session
            .lock()
            .unwrap()
            .index
            .values()
            .all(|index| index.kind != "recap")
    );
}

#[tokio::test]
async fn expired_or_foreign_candidates_never_construct_requests_and_cancel_is_not_failure() {
    let (_fixture, runner) = setup();
    runner.set_enabled(true);
    let stale = candidate(&mut runner.session.lock().unwrap());
    runner
        .session
        .lock()
        .unwrap()
        .append(super::super::format::Body::PlayerSpeech {
            player_id: "p".into(),
            text: "新输入".into(),
            mode: None,
            content_range: None,
        })
        .unwrap();
    let factory = Box::new(|_: &RecapCandidate| {
        Ok(generation(
            vec![
                Ok(ProviderDelta::Text("摘要".into())),
                Ok(ProviderDelta::Finish(ProviderFinish::Stop)),
            ],
            false,
        )
        .0)
    });
    assert!(
        runner
            .attempt(trigger(1000, true), stale, factory)
            .await
            .is_err()
    );
    let mut other = Fixture::new();
    let foreign = candidate(&mut other.session);
    assert!(
        runner
            .attempt(
                trigger(1000, true),
                foreign,
                Box::new(|_| Err(Fault::bad_request()))
            )
            .await
            .is_err()
    );
    assert!(runner.policy.lock().unwrap().last_attempt_seconds.is_none());
    let frozen = candidate(&mut runner.session.lock().unwrap());
    let (request, calls) = generation(vec![Ok(ProviderDelta::Text("不得发出".into()))], false);
    assert_eq!(
        runner
            .attempt(
                trigger(1000, true),
                frozen,
                Box::new(|_| {
                    runner.cancel();
                    Ok(request)
                })
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        runner
            .policy
            .lock()
            .unwrap()
            .eligible(1300, 60, 16, 4096, true)
    );
}

#[tokio::test]
async fn cancellation_while_validation_waits_never_constructs_a_request() {
    let (_fixture, runner) = setup();
    runner.set_enabled(true);
    let frozen = candidate(&mut runner.session.lock().unwrap());
    let session = runner.session.clone();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = session.lock().unwrap();
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    locked_rx.await.unwrap();
    let run = runner.attempt(
        trigger(1000, true),
        frozen,
        Box::new(|_| Err(Fault::bad_request())),
    );
    tokio::pin!(run);
    tokio::select! {
        result=&mut run=>panic!("验证不应越过锁：{result:?}"),
        ()=async {
            loop {
                if runner.cancel.lock().unwrap().is_some() { break; }
                tokio::task::yield_now().await;
            }
            runner.cancel();
            release_tx.send(()).unwrap();
        }=>{}
    }
    assert_eq!(run.await.unwrap(), None);
    holder.join().unwrap();
    assert!(runner.policy.lock().unwrap().last_attempt_seconds.is_none());
}

#[test]
fn only_completed_private_output_is_eligible_for_recap() {
    for outcome in [Outcome::Completed, Outcome::Cancelled, Outcome::Failed] {
        let result = crate::turn::PrivateResult {
            turn_id: uuid::Uuid::new_v4().to_string(),
            text: "正文".into(),
            outcome,
            finish_reason: None,
        };
        assert_eq!(
            completed_text(result).is_some(),
            outcome == Outcome::Completed
        );
    }
}

#[test]
fn incomplete_private_results_never_reach_the_commit_boundary() {
    let (_fixture, runner) = setup();
    let frozen = candidate(&mut runner.session.lock().unwrap());
    assert_eq!(
        commit(
            runner.session.clone(),
            CancellationToken::new(),
            frozen,
            None
        )
        .unwrap(),
        None
    );
}
