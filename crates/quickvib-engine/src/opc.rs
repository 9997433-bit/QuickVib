//! `*OPC` / `*OPC?` bookkeeping (`docs/PLAN.md` 8.1).
//!
//! A recording is the only overlapped operation QuickVib has, so "pending operations" reduces
//! to "a run is in flight".

/// The operation-complete state of the standard event register.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Opc {
    operation_in_progress: bool,
    requested: bool,
    bit_set: bool,
}

impl Opc {
    /// A register with no pending operation and the OPC bit clear.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            operation_in_progress: false,
            requested: false,
            bit_set: false,
        }
    }

    /// Mark an overlapped operation as started. Clears the OPC bit.
    pub fn begin_operation(&mut self) {
        self.operation_in_progress = true;
        self.bit_set = false;
    }

    /// Mark the overlapped operation as finished, setting the OPC bit if `*OPC` asked for it.
    pub fn end_operation(&mut self) {
        self.operation_in_progress = false;
        if self.requested {
            self.requested = false;
            self.bit_set = true;
        }
    }

    /// Handle `*OPC`: set the bit now if nothing is pending, otherwise arm it.
    pub fn request(&mut self) {
        if self.operation_in_progress {
            self.requested = true;
        } else {
            self.bit_set = true;
        }
    }

    /// Whether `*OPC?` would have to block.
    #[must_use]
    pub const fn is_operation_in_progress(&self) -> bool {
        self.operation_in_progress
    }

    /// Whether the OPC bit in the standard event register is set.
    #[must_use]
    pub const fn bit(&self) -> bool {
        self.bit_set
    }

    /// Handle `*CLS`: clear the bit and any armed request, leaving a pending operation alone.
    pub fn clear(&mut self) {
        self.requested = false;
        self.bit_set = false;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn opc_with_nothing_pending_sets_the_bit_immediately() {
        let mut opc = Opc::new();
        opc.request();
        assert!(opc.bit());
    }

    #[test]
    fn opc_during_a_run_waits_for_the_run_to_finish() {
        let mut opc = Opc::new();
        opc.begin_operation();
        opc.request();
        assert!(!opc.bit());
        assert!(opc.is_operation_in_progress());
        opc.end_operation();
        assert!(opc.bit());
        assert!(!opc.is_operation_in_progress());
    }

    #[test]
    fn starting_a_run_clears_a_previously_set_bit() {
        let mut opc = Opc::new();
        opc.request();
        assert!(opc.bit());
        opc.begin_operation();
        assert!(!opc.bit());
    }

    #[test]
    fn cls_clears_the_bit_and_the_pending_request() {
        let mut opc = Opc::new();
        opc.begin_operation();
        opc.request();
        opc.clear();
        opc.end_operation();
        assert!(!opc.bit());
    }

    #[test]
    fn end_without_a_request_leaves_the_bit_clear() {
        let mut opc = Opc::new();
        opc.begin_operation();
        opc.end_operation();
        assert!(!opc.bit());
    }
}
