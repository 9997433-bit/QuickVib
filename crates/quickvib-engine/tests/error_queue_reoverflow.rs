//! Regression coverage for repeated error-queue overflow (Round 1 B6).

use quickvib_core::error::ERROR_QUEUE_DEPTH;
use quickvib_core::ScpiError;
use quickvib_engine::ErrorQueue;

#[test]
fn second_overflow_after_partial_drain_surfaces_a_new_queue_overflow() {
    let mut queue = ErrorQueue::new();
    for _ in 0..ERROR_QUEUE_DEPTH {
        queue.push(ScpiError::CommandError);
    }
    queue.push(ScpiError::UndefinedHeader);

    assert_eq!(queue.pop().error, ScpiError::CommandError);
    queue.push(ScpiError::DataOutOfRange);
    queue.push(ScpiError::IllegalParameterValue);

    let mut drained = Vec::new();
    while !queue.is_empty() {
        drained.push(queue.pop().error);
    }

    assert_eq!(
        drained.last(),
        Some(&ScpiError::QueueOverflow),
        "the second overflow must replace the newest queued error with -350"
    );
    assert_eq!(
        drained
            .iter()
            .filter(|error| **error == ScpiError::QueueOverflow)
            .count(),
        2,
        "both distinct overflow events must remain observable"
    );
}
