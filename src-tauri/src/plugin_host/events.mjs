// Translate the host event vocabulary once, independently of any plugin.
export class SessionEvents {
  sessions = new Map();
  constructor(ctx, createSession) { this.ctx = ctx; this.createSession = createSession; }
  accept(payload) {
    this.ctx.emit('android-tool/event', payload);
    const id = payload.session_id ?? payload.sessionId;
    if (!id) return;
    const turnId = payload.turn_id ?? payload.turnId;
    let state = this.sessions.get(id);
    if (!state) {
      state = { session: this.createSession(id, { title: payload.title ?? id, origin: 'user' }), turnId: undefined, turn: 0, ended: false };
      this.sessions.set(id, state);
      this.ctx.emit('session/created', state.session);
    }
    const event = (type, data) => this.ctx.emit('session/event', state.session,
      state.session.append(type, data, type === 'tool/result' ? { surfaceOp: 'append' } : undefined));
    if (turnId && state.turnId !== turnId) {
      state.turnId = turnId; state.turn += 1; state.ended = false;
      event('turn/start', { turn: state.turn });
    }
    switch (payload.type) {
      case 'reasoning_delta': case 'text_delta':
        if (!state.ended) this.ctx.emit('agent/assistant-stream', { agent: { session: state.session },
          frame: { type: 'chunk', chunk: { type: payload.type === 'reasoning_delta' ? 'reasoning-delta' : 'text-delta', text: payload.text ?? '' } } });
        break;
      case 'tool_call_started':
        event('tool/call', { callId: payload.call_id ?? payload.callId, name: payload.tool_name ?? payload.toolName,
          arguments: typeof (payload.input ?? payload.arguments) === 'string' ? payload.input ?? payload.arguments
            : JSON.stringify(payload.input ?? payload.arguments ?? payload.tool_input ?? {}) });
        break;
      case 'tool_call_finished':
        event('tool/result', { message: { role: 'tool', toolCallId: payload.call_id ?? payload.callId,
          isError: payload.is_error === true || payload.status === 'error', content: payload.output ?? payload.result ?? '' },
          ...(payload.error ? { error: payload.error } : {}) });
        break;
      case 'permission_requested': case 'user_input_requested':
        event('turn/end', { turn: state.turn, reason: { kind: 'blocked' }, request: payload }); break;
      case 'done': case 'completed': case 'error': case 'cancelled':
        if (state.ended) break;
        state.ended = true;
        event('turn/end', { turn: state.turn, reason: { kind: payload.type === 'error' ? 'error' : payload.type === 'cancelled' ? 'aborted' : 'completed' } });
        break;
    }
  }
}
