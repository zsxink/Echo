use super::*;

#[test]
fn a_volume_change_while_paused_reaches_the_store() {
    // 用户暂停后调音量：暂停态下没有后续快照，位置推进的节流也够不着，
    // 所以旧策略（"下一个快照才写，且受 min_interval 约束"）永远写不进去
    // —— 存下的还是调整前的音量 / `muted:true`，下次启动被原样重放。
    let (port, store) = paused_fake_with_saver(0.5);
    // Two transitions (Playing, then Paused) are each written at once —
    // that part always worked. Wait for both so the assertion below is
    // about the volume change alone.
    assert!(
        wait_until(|| store.saves() >= 2, std::time::Duration::from_secs(3)),
        "the transport transitions must be saved [saves={}]",
        store.saves()
    );
    let after_transition = store.saves();

    port.send(PlayerCommand::SetVolume(0.8))
        .expect("set volume");

    // The write must happen once the burst settles (~250 ms) with no
    // further transition and no position progress to carry it.
    assert!(
        wait_until(
            || store.saves() > after_transition,
            std::time::Duration::from_secs(3)
        ),
        "暂停态下调的音量必须落盘，否则下次启动重放的是旧音量"
    );
    let persisted = store.last().expect("a session was stored");
    assert!(
        (persisted.volume - 0.8).abs() < 1e-9,
        "落盘的必须是最新的音量"
    );
}

#[test]
fn a_slider_drag_is_collapsed_into_one_save_of_its_final_value() {
    // Dragging the volume slider publishes a burst of snapshots. Writing
    // each one would fsync ~10×/second; the settle window collapses the
    // burst into a single save — and it must be the value the user let go
    // on, not one from the middle of the drag.
    let (port, store) = paused_fake_with_saver(0.5);
    assert!(
        wait_until(|| store.saves() >= 2, std::time::Duration::from_secs(3)),
        "the transport transitions must be saved first"
    );

    for step in [0.6, 0.7, 0.75, 0.9] {
        port.send(PlayerCommand::SetVolume(step))
            .expect("drag step");
        // Faster than the settle window: one continuous drag.
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    assert!(
        wait_until(
            || store.last().map(|s| s.volume) == Some(0.9),
            std::time::Duration::from_secs(3)
        ),
        "落盘的必须是松手时的最终音量 [stored={:?}]",
        store.last().map(|s| s.volume)
    );
}
