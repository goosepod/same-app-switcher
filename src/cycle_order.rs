//! Maintain a stable ring while our own activations change Windows' z-order.

pub struct CycleOrder<T> {
    order: Vec<T>,
    expected_foreground: Option<T>,
}

impl<T: Copy + Eq> CycleOrder<T> {
    pub const fn new() -> Self {
        Self {
            order: Vec::new(),
            expected_foreground: None,
        }
    }

    pub fn target(&mut self, live: &[T], current: T, reverse: bool) -> Option<T> {
        if self.expected_foreground != Some(current) {
            // A mouse click, Alt+Tab, or a different app starts a fresh MRU ring.
            self.order = live.to_vec();
        } else {
            self.order.retain(|window| live.contains(window));
            for &window in live {
                if !self.order.contains(&window) {
                    self.order.push(window);
                }
            }
        }
        self.expected_foreground = Some(current);
        if self.order.len() < 2 {
            return None;
        }
        let index = self.order.iter().position(|&window| window == current)?;
        let offset = if reverse { self.order.len() - 1 } else { 1 };
        Some(self.order[(index + offset) % self.order.len()])
    }

    // Commit only when Windows actually focused the requested window.
    pub fn activated(&mut self, window: T) {
        self.expected_foreground = Some(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_every_window_despite_activation_reordering() {
        let mut ring = CycleOrder::new();
        assert_eq!(ring.target(&[1, 2, 3], 1, false), Some(2));
        ring.activated(2);
        assert_eq!(ring.target(&[2, 1, 3], 2, false), Some(3));
        ring.activated(3);
        assert_eq!(ring.target(&[3, 2, 1], 3, false), Some(1));
    }

    #[test]
    fn reversing_undoes_the_previous_step_and_wraps() {
        let mut ring = CycleOrder::new();
        assert_eq!(ring.target(&[1, 2, 3], 1, false), Some(2));
        ring.activated(2);
        assert_eq!(ring.target(&[2, 1, 3], 2, true), Some(1));
        ring.activated(1);
        assert_eq!(ring.target(&[1, 2, 3], 1, true), Some(3));
    }

    #[test]
    fn removes_closed_windows_and_appends_new_windows() {
        let mut ring = CycleOrder::new();
        ring.target(&[1, 2, 3], 1, false);
        ring.activated(2);
        assert_eq!(ring.target(&[2, 4, 1], 2, false), Some(4));
        ring.activated(4);
        assert_eq!(ring.target(&[4, 2, 1], 4, false), Some(1));
    }

    #[test]
    fn manual_focus_change_starts_a_new_ring() {
        let mut ring = CycleOrder::new();
        ring.target(&[1, 2, 3], 1, false);
        ring.activated(2);
        assert_eq!(ring.target(&[3, 1, 2], 3, false), Some(1));
    }

    #[test]
    fn failed_activation_does_not_advance() {
        let mut ring = CycleOrder::new();
        assert_eq!(ring.target(&[1, 2, 3], 1, false), Some(2));
        assert_eq!(ring.target(&[1, 2, 3], 1, false), Some(2));
    }

    #[test]
    fn no_target_for_missing_or_single_foreground_window() {
        let mut ring = CycleOrder::new();
        assert_eq!(ring.target(&[], 1, false), None);
        assert_eq!(ring.target(&[1], 1, true), None);
        assert_eq!(ring.target(&[2, 3], 1, false), None);
    }

    #[test]
    fn process_identity_prevents_reusing_a_closed_window_handle() {
        let mut ring = CycleOrder::new();
        ring.target(&[(1, 10), (2, 20), (3, 30)], (1, 10), false);
        ring.activated((1, 10));
        assert_eq!(
            ring.target(&[(1, 10), (2, 99), (3, 30)], (1, 10), false),
            Some((3, 30))
        );
    }
}
