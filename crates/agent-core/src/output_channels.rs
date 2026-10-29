use crate::AgentEvent;

/// Some compatible gateways put untagged execution planning into output_text.
/// Hold only a possible planning header; ordinary English and code pass through.
#[derive(Default)]
pub(crate) struct OutputChannels {
    pending: String,
    decided: bool,
    reasoning: bool,
    fenced: bool,
}

impl OutputChannels {
    pub(crate) fn push(&mut self, session_id: &str, text: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for character in text.chars() {
            self.pending.push(character);
            if !self.decided {
                let prefix = self.pending.trim_start();
                if !self.fenced && prefix.starts_with("The user wants me to ") {
                    if prefix.contains("Based on the conversation, I need to:") {
                        self.reasoning = true;
                        self.decided = true;
                    } else if character != '\n' && prefix.len() < 256 {
                        continue;
                    }
                } else if !self.fenced
                    && "The user wants me to ".starts_with(prefix)
                    && character != '\n'
                {
                    continue;
                }
                if prefix.starts_with("```") || prefix.starts_with("~~~") {
                    self.fenced = !self.fenced;
                    self.decided = true;
                } else if ("```".starts_with(prefix) || "~~~".starts_with(prefix))
                    && character != '\n'
                {
                    continue;
                } else if self.reasoning && !self.fenced {
                    let prose = prefix.trim_start_matches(['*', '#', '-', ' ']);
                    if prose.chars().next().is_some_and(is_han) {
                        if prose.chars().filter(|character| is_han(*character)).count() >= 4 {
                            self.reasoning = false;
                        } else if character != '\n' && prose.len() < 96 {
                            continue;
                        }
                    }
                }
                self.decided = true;
            }
            append(
                &mut events,
                session_id,
                self.reasoning,
                std::mem::take(&mut self.pending),
            );
            if character == '\n' {
                self.decided = false;
            }
        }
        events
    }

    pub(crate) fn finish(&mut self, session_id: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        append(
            &mut events,
            session_id,
            self.reasoning,
            std::mem::take(&mut self.pending),
        );
        self.decided = false;
        events
    }
}

fn is_han(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{20000}'..='\u{3134f}')
}

pub(crate) fn public_history_text(text: &str) -> String {
    let mut channels = OutputChannels::default();
    let mut events = channels.push("history", text);
    events.extend(channels.finish("history"));
    let mut public = String::new();
    for event in events {
        if let AgentEvent::TextDelta { text, .. } = event {
            public.push_str(&text);
        }
    }
    public
}

fn append(events: &mut Vec<AgentEvent>, session_id: &str, reasoning: bool, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(AgentEvent::ReasoningDelta { text: previous, .. }) = events.last_mut() {
        if reasoning {
            previous.push_str(&text);
            return;
        }
    }
    if let Some(AgentEvent::TextDelta { text: previous, .. }) = events.last_mut() {
        if !reasoning {
            previous.push_str(&text);
            return;
        }
    }
    events.push(if reasoning {
        AgentEvent::ReasoningDelta {
            session_id: session_id.to_string(),
            text,
        }
    } else {
        AgentEvent::TextDelta {
            session_id: session_id.to_string(),
            text,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partition(text: &str, character_chunks: bool) -> (String, String) {
        let mut filter = OutputChannels::default();
        let mut events = Vec::new();
        if character_chunks {
            for character in text.chars() {
                events.extend(filter.push("test", &character.to_string()));
            }
        } else {
            events.extend(filter.push("test", text));
        }
        events.extend(filter.finish("test"));
        let mut public = String::new();
        let mut reasoning = String::new();
        for event in events {
            match event {
                AgentEvent::TextDelta { text, .. } => public.push_str(&text),
                AgentEvent::ReasoningDelta { text, .. } => reasoning.push_str(&text),
                _ => unreachable!(),
            }
        }
        (public, reasoning)
    }

    #[test]
    fn untagged_planning_stays_out_of_public_text_across_chunk_boundaries() {
        let planning = "The user wants me to continue. Based on the conversation, I need to:\n\n1. Check device status\n2. Use tools\n\nLet me follow the instructions:\n- Chinese responses\n";
        let input =
            format!("进程已拉起，接着核对日志。\n{planning}\n已完成检查，当前没有匹配日志。\n");
        for chunks in [false, true] {
            let (public, reasoning) = partition(&input, chunks);
            assert_eq!(
                public,
                "进程已拉起，接着核对日志。\n已完成检查，当前没有匹配日志。\n"
            );
            assert_eq!(reasoning, format!("{planning}\n"));
        }
    }

    #[test]
    fn preserves_english_answers_quotes_code_and_incomplete_headers() {
        for text in [
            "The user wants me to explain the API. Here is the answer.\nEnglish is a valid response.",
            "> The user wants me to continue. Based on the conversation, I need to:\nThis is a quote.",
            "```text\nThe user wants me to continue. Based on the conversation, I need to:\n```\nNormal answer.",
            "The user wants me to ",
            "  Text with whitespace\n\n**Bold** and `code`.",
        ] {
            for chunks in [false, true] {
                assert_eq!(partition(text, chunks), (text.to_string(), String::new()));
            }
        }
    }
}
