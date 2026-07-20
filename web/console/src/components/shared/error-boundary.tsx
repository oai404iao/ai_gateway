import { Component, type ReactNode } from "react";
import { AlertCircle } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

interface Props {
  children: ReactNode;
}
interface State {
  error: Error | null;
}

/** Catches render-time errors so a single broken page does not blank the SPA. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: { componentStack: string | null }) {
    console.error("Unhandled render error", error, info);
  }

  reset = () => this.setState({ error: null });

  render() {
    if (this.state.error) {
      return (
        <div className="flex flex-col gap-4 p-6">
          <Alert variant="destructive">
            <AlertCircle data-icon="inline-start" />
            <AlertTitle>Something went wrong</AlertTitle>
            <AlertDescription>{this.state.error.message}</AlertDescription>
          </Alert>
          <Button variant="outline" className="self-start" onClick={this.reset}>
            Try again
          </Button>
        </div>
      );
    }
    return this.props.children;
  }
}
