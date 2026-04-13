use crate::{llama_client::FimTimingsData, FimResponseWithInfo};

#[derive(Debug, Clone, Default)]
pub struct FimState {
    pub hint_shown: bool,

    pub pos_x: usize,
    pub pos_y: usize,
    pub line_cur: String,
    pub content: Vec<String>,
    /// Timing data from the last completion for display in info string
    pub timings: Option<FimTimingsData>,
    /// Collection of completions for cycling (longest to shortest)
    pub completion_cycle: Vec<FimResponseWithInfo>,
    /// Index of currently displayed completion in the cycle
    pub completion_index: usize,
}

impl FimState {
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub fn update(
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

    pub fn clear(&mut self) {
        self.hint_shown = false;
        self.pos_x = 0;
        self.pos_y = 0;
        self.line_cur.clear();
        self.content.clear();
        self.timings = None;
        self.completion_cycle.clear();
        self.completion_index = 0;
    }

    /// Set the completion cycle list
    #[allow(dead_code)]
    pub fn set_completion_cycle(&mut self, completions: Vec<FimResponseWithInfo>, idx: usize) {
        self.completion_cycle = completions;
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    pub fn set_completion_idx(&mut self, idx: usize) {
        self.completion_index = idx;
    }

    /// Set the completion cycle list
    pub fn push_completion_cycle_if_unique(&mut self, completions: FimResponseWithInfo) -> bool {
        // first check if this completion's contents are unique
        if self
            .completion_cycle
            .iter()
            .any(|c| c.resp.content == completions.resp.content)
        {
            return false;
        }

        self.completion_cycle.push(completions);
        true
    }

    pub fn push_completion_idx_to_tail(&mut self) {
        self.set_completion_idx(self.completion_cycle.len() - 1);
    }

    /// Cycle to next completion
    pub fn cycle_next(&mut self) -> Option<FimResponseWithInfo> {
        if self.completion_cycle.is_empty() {
            return None;
        }
        self.completion_index = (self.completion_index + 1) % self.completion_cycle.len();
        // Update content to match the current completion
        let current = &self.completion_cycle[self.completion_index];
        Some(current.clone())
    }

    /// Cycle to previous completion
    pub fn cycle_prev(&mut self) -> Option<FimResponseWithInfo> {
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
