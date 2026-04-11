#[derive(Debug, Clone, Default)]
pub struct FimState {
    hint_shown: bool,
    /// Last buffer id and cursor Y position where ring buffer chunks were picked
    last_pick_buf_id_pos_y: Option<(u64, usize)>,

    pos_x: usize,
    pos_y: usize,
    line_cur: String,
    content: Vec<String>,
    /// Timing data from the last completion for display in info string
    timings: Option<FimTimingsData>,
    /// Collection of completions for cycling (longest to shortest)
    completion_cycle: Vec<FimResponse>,
    /// Index of currently displayed completion in the cycle
    completion_index: usize,
}

impl FimState {
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    fn update(
        &mut self,
        hint_shown: bool,
        pos_x: usize,
        pos_y: usize,
        line_cur: String,
        content: Vec<String>,
        timings: Option<FimTimingsData>,
    ) {
        self.hint_shown = hint_shown;
        self.pos_x = pos_x;
        self.pos_y = pos_y;
        self.line_cur = line_cur;
        self.content.clear();
        self.content = content;
        self.timings = timings;
    }

    fn clear(&mut self) {
        self.hint_shown = false;
        self.pos_x = 0;
        self.pos_y = 0;
        self.line_cur.clear();
        self.content.clear();
        self.last_pick_buf_id_pos_y = None;
        self.timings = None;
        self.completion_cycle.clear();
        self.completion_index = 0;
    }

    /// Update the last pick position
    fn set_last_pick_buf_id_pos_y(&mut self, buf_id: u64, pos_y: usize) {
        self.last_pick_buf_id_pos_y = Some((buf_id, pos_y));
    }

    /// Get the last pick position
    fn get_last_pick_buf_id_pos_y(&self) -> Option<(u64, usize)> {
        self.last_pick_buf_id_pos_y
    }

    /// Set the completion cycle list
    #[allow(dead_code)]
    fn set_completion_cycle(&mut self, completions: Vec<FimResponse>, idx: usize) {
        self.completion_cycle = completions;
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    fn set_completion_idx(&mut self, idx: usize) {
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    fn push_completion_cycle_if_unique(&mut self, completions: FimResponse) -> bool {
        // first check if this completion's contents are unique
        if self
            .completion_cycle
            .iter()
            .any(|c| c.content == completions.content)
        {
            return false;
        }

        self.completion_cycle.push(completions);
        true
    }

    fn push_completion_idx_to_tail(&mut self) {
        self.set_completion_idx(self.completion_cycle.len() - 1);
    }

    /// Cycle to next completion
    fn cycle_next(&mut self) -> Option<FimResponse> {
        if self.completion_cycle.is_empty() {
            return None;
        }
        self.completion_index = (self.completion_index + 1) % self.completion_cycle.len();
        // Update content to match the current completion
        let current = &self.completion_cycle[self.completion_index];
        Some(current.clone())
    }

    /// Cycle to previous completion
    fn cycle_prev(&mut self) -> Option<FimResponse> {
        if self.completion_cycle.is_empty() {
            return None;
        }
        self.completion_index = if self.completion_index == 0 {
            self.completion_cycle.len() - 1
        } else {
            self.completion_index - 1
        };
        // Update content to match the current completion
        let current = &self.completion_cycle[self.completion_index];
        Some(current.clone())
    }
}
