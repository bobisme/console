use console_core::{Console, MAX_PLATFORM_EVENTS, MAX_SCORE, PlatformEvent};

fn cart(lua: &str) -> Console {
    Console::new(&format!("__lua__\n{lua}\n"), 0).unwrap()
}

#[test]
fn score_and_leaderboard_events_are_ordered_and_drained() {
    let mut console = cart(&format!(
        r#"
score_update(12)
score_update(12)
function _update()
  score_update({MAX_SCORE})
  score_submit()
  score_submit()
  leaderboard_show()
end
"#
    ));

    assert_eq!(
        console.take_platform_events().events,
        [PlatformEvent::ScoreUpdate { score: 12 }]
    );
    console.step(0).unwrap();
    assert_eq!(
        console.take_platform_events().events,
        [
            PlatformEvent::ScoreUpdate { score: MAX_SCORE },
            PlatformEvent::ScoreSubmit { score: MAX_SCORE },
            PlatformEvent::LeaderboardShow,
        ]
    );
    assert!(console.take_platform_events().events.is_empty());
}

#[test]
fn post_submit_update_starts_a_new_result_even_at_the_same_score() {
    let mut console = cart(
        r#"
function _update()
  score_update(7)
  score_submit()
  score_update(7)
  score_submit()
end
"#,
    );
    console.step(0).unwrap();
    assert_eq!(
        console.take_platform_events().events,
        [
            PlatformEvent::ScoreUpdate { score: 7 },
            PlatformEvent::ScoreSubmit { score: 7 },
            PlatformEvent::ScoreUpdate { score: 7 },
            PlatformEvent::ScoreSubmit { score: 7 },
        ]
    );
}

#[test]
fn submit_before_update_is_zero_and_api_has_no_readback() {
    let mut console = cart(
        r#"
returns = {score_submit(), leaderboard_show()}
"#,
    );
    assert_eq!(
        console.take_platform_events().events,
        [
            PlatformEvent::ScoreSubmit { score: 0 },
            PlatformEvent::LeaderboardShow,
        ]
    );
    assert_eq!(console.get_global("score_best").unwrap().type_name(), "nil");
    let returns = console.get_global("returns").unwrap();
    assert_eq!(returns.as_table().unwrap().raw_len(), 0);
}

#[test]
fn score_domain_rejects_fractional_negative_non_finite_and_oversized_values() {
    for value in [
        "-1".to_string(),
        "1.5".to_string(),
        "0/0".to_string(),
        "1/0".to_string(),
        (MAX_SCORE + 1).to_string(),
        "'10'".to_string(),
    ] {
        let error = Console::new(&format!("__lua__\nscore_update({value})\n"), 0).unwrap_err();
        assert!(
            error.message().contains("integer in 0..=9007199254740991"),
            "{value}: {error}"
        );
    }
}

#[test]
fn unobserved_platform_event_stream_is_bounded_and_reports_drops() {
    let mut console = cart(&format!(
        "for i=1,{} do leaderboard_show() end",
        MAX_PLATFORM_EVENTS + 17
    ));
    let frame = console.take_platform_events();
    assert_eq!(frame.capacity, MAX_PLATFORM_EVENTS);
    assert_eq!(frame.events.len(), MAX_PLATFORM_EVENTS);
    assert_eq!(frame.dropped, 17);
    let empty = console.take_platform_events();
    assert!(empty.events.is_empty());
    assert_eq!(empty.dropped, 0);
}

#[test]
fn failed_frames_and_evals_do_not_commit_external_events_or_score_state() {
    let mut console = cart(
        r#"
score_update(5)
function _update()
  score_update(99)
  score_submit()
  error("frame failed")
end
"#,
    );
    assert!(console.step(0).is_err());
    assert_eq!(
        console.take_platform_events().events,
        [PlatformEvent::ScoreUpdate { score: 5 }],
        "the earlier successful init boundary remains, failed-frame effects do not"
    );

    let mut console = cart("score_update(5)");
    let _ = console.take_platform_events();
    assert!(
        console
            .eval("score_update(99); score_submit(); error('eval failed')")
            .is_err()
    );
    assert!(console.take_platform_events().events.is_empty());
    console.eval("score_submit()").unwrap();
    assert_eq!(
        console.take_platform_events().events,
        [PlatformEvent::ScoreSubmit { score: 5 }],
        "failed eval must roll score state back as well as suppressing events"
    );
}

#[test]
fn failed_dev_hook_does_not_commit_platform_events() {
    let mut console = cart(
        r#"
devhook.register("fail", {
  description="fail after submit",
  phase="post_frame",
  run=function() score_update(88) score_submit() error("hook failed") end,
})
"#,
    );
    assert!(
        console
            .invoke_dev_hook(
                "fail",
                console_core::DevHookPhase::PostFrame,
                &console_core::DevValue::Null,
            )
            .is_err()
    );
    assert!(console.take_platform_events().events.is_empty());
}
