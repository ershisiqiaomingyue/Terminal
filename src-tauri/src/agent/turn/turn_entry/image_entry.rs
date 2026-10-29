    let current_user_message_id = format!("user:{turn_id}");
    let attachment_root = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("agent-attachments");
    let mut image_attachments = normalize_request_image_attachments(
        &attachment_root,
        &request.attachments,
        &request.context,
        selected_model.supports_vision,
        &current_user_message_id,
    )?;
    for turn in &mut request.context {
