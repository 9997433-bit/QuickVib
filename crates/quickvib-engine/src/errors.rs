//! The instrument error queue (`docs/PLAN.md` 8.1).
//!
//! FIFO, bounded at [`quickvib_core::error::ERROR_QUEUE_DEPTH`]. Overflow replaces the last
//! entry with `-350,"Queue overflow"` rather than dropping the oldest, so the UTS always
//! learns that it lost errors.

use std::collections::VecDeque;

use quickvib_core::error::ERROR_QUEUE_DEPTH;
use quickvib_core::{ScpiError, ScpiErrorEntry};

/// A bounded FIFO of pending errors.
#[derive(Debug, Default)]
pub struct ErrorQueue {
    entries: VecDeque<ScpiErrorEntry>,
    overflowed: bool,
}

impl ErrorQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push an error, collapsing to `-350` once the queue is full.
    pub fn push(&mut self, entry: impl Into<ScpiErrorEntry>) {
        let entry = entry.into();
        if self.entries.len() < ERROR_QUEUE_DEPTH {
            self.entries.push_back(entry);
            return;
        }
        if !self.overflowed {
            self.overflowed = true;
            self.entries.pop_back();
            self.entries
                .push_back(ScpiErrorEntry::new(ScpiError::QueueOverflow));
        }
    }

    /// Pop the oldest error, or [`ScpiError::NoError`] when the queue is empty.
    pub fn pop(&mut self) -> ScpiErrorEntry {
        match self.entries.pop_front() {
            Some(entry) => {
                if self.entries.is_empty() {
                    self.overflowed = false;
                }
                entry
            }
            None => {
                self.overflowed = false;
                ScpiErrorEntry::new(ScpiError::NoError)
            }
        }
    }

    /// Discard every pending error. This is what `*CLS` does.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.overflowed = false;
    }

    /// How many errors are pending.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn an_empty_queue_reports_no_error() {
        let mut queue = ErrorQueue::new();
        assert_eq!(queue.pop().wire_form(), "0,\"No error\"");
        assert!(queue.is_empty());
    }

    #[test]
    fn entries_come_back_in_fifo_order() {
        let mut queue = ErrorQueue::new();
        queue.push(ScpiError::CommandError);
        queue.push(ScpiError::UndefinedHeader);
        queue.push(ScpiError::DataOutOfRange);
        assert_eq!(queue.pop().error, ScpiError::CommandError);
        assert_eq!(queue.pop().error, ScpiError::UndefinedHeader);
        assert_eq!(queue.pop().error, ScpiError::DataOutOfRange);
        assert_eq!(queue.pop().error, ScpiError::NoError);
    }

    #[test]
    fn clear_empties_the_queue() {
        let mut queue = ErrorQueue::new();
        queue.push(ScpiError::CommandError);
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.pop().error, ScpiError::NoError);
    }

    #[test]
    fn overflow_replaces_the_last_entry() {
        let mut queue = ErrorQueue::new();
        for _ in 0..ERROR_QUEUE_DEPTH {
            queue.push(ScpiError::CommandError);
        }
        assert_eq!(queue.len(), ERROR_QUEUE_DEPTH);
        queue.push(ScpiError::UndefinedHeader);
        queue.push(ScpiError::UndefinedHeader);
        assert_eq!(queue.len(), ERROR_QUEUE_DEPTH);

        for _ in 0..ERROR_QUEUE_DEPTH - 1 {
            assert_eq!(queue.pop().error, ScpiError::CommandError);
        }
        assert_eq!(queue.pop().error, ScpiError::QueueOverflow);
        assert_eq!(queue.pop().error, ScpiError::NoError);
    }

    #[test]
    fn details_are_carried_for_the_log_but_not_the_wire() {
        let mut queue = ErrorQueue::new();
        queue.push(ScpiError::IllegalParameterValue.detail("FORM XML"));
        let entry = queue.pop();
        assert_eq!(entry.wire_form(), "-224,\"Illegal parameter value\"");
        assert_eq!(entry.detail.as_deref(), Some("FORM XML"));
    }

    #[test]
    fn the_queue_recovers_after_being_drained() {
        let mut queue = ErrorQueue::new();
        for _ in 0..ERROR_QUEUE_DEPTH + 5 {
            queue.push(ScpiError::CommandError);
        }
        while !queue.is_empty() {
            let _ = queue.pop();
        }
        queue.push(ScpiError::DataCorruptOrStale);
        assert_eq!(queue.pop().error, ScpiError::DataCorruptOrStale);
    }
}
