import { useEffect, useRef } from "react";
import {
  APP_PAGE_ACTION_EVENT,
  subscribeApplicationPageAction,
  type ApplicationActionInvocation,
  unsubscribeApplicationPageAction,
} from "@/application/pageActions";

/**
 * The shared entry point for a page to receive a user-confirmed AI plan.
 * A page decides how much of the typed context it can safely prefill; it never
 * authorizes a side effect merely because an AI plan supplied parameters.
 */
export function useApplicationPageAction(actionId: string, onAction: (action: ApplicationActionInvocation) => void): void {
  const onActionRef = useRef(onAction);
  onActionRef.current = onAction;

  useEffect(() => {
    const receive = (event: Event) => {
      const action = (event as CustomEvent<ApplicationActionInvocation>).detail;
      if (!action || action.actionId !== actionId) return;
      onActionRef.current(action);
    };
    window.addEventListener(APP_PAGE_ACTION_EVENT, receive);
    const pending = subscribeApplicationPageAction(actionId);
    for (const action of pending) onActionRef.current(action);
    return () => {
      window.removeEventListener(APP_PAGE_ACTION_EVENT, receive);
      unsubscribeApplicationPageAction(actionId);
    };
  }, [actionId]);
}
