/// Drives a recovery wait from a script of events, each sent after its delay.
async fn scripted_recovery(
    script: Vec<(u64, RecoveryEvent)>,
    generation: impl std::future::Future<Output = GeneratedReport>,
    readiness: Readiness,
) -> (Option<GeneratedReport>, Vec<RecoveryNotice>) {
    let (result, notices, _) = timed_scripted_recovery(script, generation, readiness).await;
    (result, notices)
}

async fn timed_scripted_recovery(
    script: Vec<(u64, RecoveryEvent)>,
    generation: impl std::future::Future<Output = GeneratedReport>,
    readiness: Readiness,
) -> (Option<GeneratedReport>, Vec<RecoveryNotice>, Duration) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut fixture = RecoveryFixture::new(rx);
    let started = tokio::time::Instant::now();
    let sender = async move {
        for (delay, event) in script {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            let _ = tx.send(event);
        }
        tx
    };
    // The sender can finish later than the holder and hide an early return.
    let ((result, returned_at), _tx) = tokio::join!(
        async {
            let result = recover_report(
                &mut fixture,
                None,
                generation,
                REPORT_RECOVERY_WINDOW,
                std::time::Duration::ZERO,
                tokio::time::Instant::now(),
                move || readiness,
            )
            .await;
            (result, started.elapsed())
        },
        sender,
    );
    (result, fixture.1, returned_at)
}

#[tokio::test(start_paused = true)]
async fn a_late_rejoin_fits_the_browser_report_wait() {
    let generation = async {
        tokio::time::sleep(Duration::from_secs(124)).await;
        Ok(Ok(serde_json::json!({"result": "complete"})))
    };
    let (result, notices, returned_at) = timed_scripted_recovery(
        vec![
            (0, RecoveryEvent::Retry),
            (123, RecoveryEvent::Away),
            (29, RecoveryEvent::Back),
        ],
        generation,
        Readiness::Ready,
    )
    .await;
    assert_eq!(result.unwrap().unwrap().unwrap()["result"], "complete");
    assert_eq!(notices, [RecoveryNotice::Accepted]);
    assert_eq!(returned_at, Duration::from_secs(152));
    let limits: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/report-recovery.json")).unwrap();
    let browser_wait = Duration::from_secs(limits["retryWaitSeconds"].as_u64().unwrap());
    let delivery = DELIVERY_WAIT * u32::try_from(DELIVERY_ATTEMPTS).unwrap();
    assert!(
        browser_wait > returned_at + delivery,
        "browser wait {browser_wait:?} must cover holder return {returned_at:?} and delivery {delivery:?}"
    );
}
