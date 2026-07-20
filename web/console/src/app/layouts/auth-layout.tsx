import { Outlet, Link } from "react-router";

export function AuthLayout() {
  return (
    <div className="flex min-h-svh flex-col items-center justify-center gap-8 bg-muted/30 p-6">
      <Link to="/login" className="flex items-center gap-2">
        <div className="flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground">
          <span className="text-sm font-bold">AG</span>
        </div>
        <div className="flex flex-col">
          <span className="text-base font-semibold">AI Gateway</span>
          <span className="text-xs text-muted-foreground">Console</span>
        </div>
      </Link>
      <div className="w-full max-w-sm">
        <Outlet />
      </div>
    </div>
  );
}
