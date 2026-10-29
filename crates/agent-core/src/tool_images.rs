//! Rich tool observations are kept out of textual results and UI/log payloads.
use crate::{AgentImageAttachment, AgentMessage, MessageRole};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

pub fn tool_image_owner(call_id: &str) -> String {
    format!("tool-result:{call_id}")
}

pub fn message_owns_image(message: &AgentMessage, owner: &str) -> bool {
    match message.role {
        MessageRole::User => message.source_message_id.as_deref() == Some(owner),
        MessageRole::Tool => message
            .tool_call_id
            .as_deref()
            .is_some_and(|id| tool_image_owner(id) == owner),
        _ => false,
    }
}

pub(crate) fn extract_tool_images(output: &mut Value) -> Vec<AgentImageAttachment> {
    fn visit(value: &mut Value, images: &mut Vec<AgentImageAttachment>, bytes: &mut usize) {
        if value["type"] == "image" && value.get("data").is_some() {
            let data = value["data"].as_str().unwrap_or_default();
            let mime = value["mimeType"]
                .as_str()
                .or_else(|| value["mediaType"].as_str())
                .unwrap_or_default();
            let valid = matches!(
                mime,
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            ) && !data.is_empty()
                && data.len() <= crate::MAX_AGENT_IMAGE_DATA_URL_CHARS - 128
                && *bytes + data.len() <= 16 * 1024 * 1024
                && images.len() < 8
                && STANDARD.decode(data).is_ok();
            let index = images.len();
            if valid {
                *bytes += data.len();
                images.push(AgentImageAttachment {
                    name: format!(
                        "tool-image-{}.{}",
                        index + 1,
                        mime.strip_prefix("image/").unwrap()
                    ),
                    media_type: mime.into(),
                    data_url: format!("data:{mime};base64,{data}"),
                    path: None,
                });
            }
            *value = json!({"type":"image","mimeType":mime,"imageIndex":if valid {Some(index)} else {None},"status":if valid {"captured"} else {"rejected: invalid, unsupported, or over image budget"}});
            return;
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, images, bytes);
                }
            }
            Value::Object(values) => {
                for value in values.values_mut() {
                    visit(value, images, bytes);
                }
            }
            _ => {}
        }
    }
    let mut images = Vec::new();
    visit(output, &mut images, &mut 0);
    images
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rich_images_are_separated_even_in_nested_plugin_results() {
        let mut output = json!({"value":{"content":[{"type":"image","mimeType":"image/png","data":"AQID"},{"type":"text","text":"observed DOM"},{"type":"image","mimeType":"image/png","data":"not base64"}]}});
        let images = extract_tool_images(&mut output);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data_url, "data:image/png;base64,AQID");
        assert!(!output.to_string().contains("AQID"));
        assert!(!output.to_string().contains("not base64"));
        assert!(output.to_string().contains("observed DOM"));
    }
}
