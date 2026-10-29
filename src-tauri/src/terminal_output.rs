use serde::Serialize;
use std::collections::VecDeque;

const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub sequence: u64,
    pub data: String,
}

#[derive(Default)]
pub struct TerminalOutput {
    sequence: u64,
    bytes: usize,
    chunks: VecDeque<OutputChunk>,
}

impl TerminalOutput {
    pub fn append(&mut self, data: String) -> OutputChunk {
        self.sequence += 1;
        let chunk = OutputChunk {
            sequence: self.sequence,
            data,
        };
        self.bytes += chunk.data.len();
        self.chunks.push_back(chunk.clone());
        while self.bytes > MAX_BYTES && self.chunks.len() > 1 {
            self.bytes -= self.chunks.pop_front().unwrap().data.len();
        }
        chunk
    }

    pub fn since(&self, sequence: u64) -> Vec<OutputChunk> {
        self.chunks
            .iter()
            .filter(|chunk| chunk.sequence > sequence)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handoff_replays_only_missing_output_and_bounds_memory() {
        let mut output = TerminalOutput::default();
        assert_eq!(output.append("first".into()).sequence, 1);
        assert_eq!(output.append("second".into()).sequence, 2);
        assert_eq!(output.since(1)[0].data, "second");
        assert!(output.since(2).is_empty());
        for _ in 0..100 {
            output.append("x".repeat(16 * 1024));
        }
        assert!(output.bytes <= MAX_BYTES);
        assert_eq!(output.since(0).last().unwrap().sequence, 102);
    }
}
