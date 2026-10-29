import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/Button";
import { reportFrontendError } from "@/platform/frontendHealth";
import "./app-recovery.css";

export function AppRecovery({ title, message, loading = false }: { title: string; message: string; loading?: boolean }) {
  return <section className="app-recovery" role={loading ? "status" : "alert"}>
    <div className="app-recovery-content">
      <h1>{title}</h1>
      <p>{message}</p>
      {!loading && <Button onClick={() => window.location.reload()}>重新加载</Button>}
    </div>
  </section>;
}

export class AppErrorBoundary extends Component<{ children: ReactNode; resetKey?: string }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) { return { error }; }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("页面加载或渲染失败", error, info.componentStack);
    reportFrontendError("react-render", error);
  }

  componentDidUpdate(previous: Readonly<{ children: ReactNode; resetKey?: string }>) {
    if (this.state.error && previous.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  render() {
    return this.state.error
      ? <AppRecovery title="页面加载失败" message="可以切换到其他页面继续使用，或重新加载后重试。" />
      : this.props.children;
  }
}
