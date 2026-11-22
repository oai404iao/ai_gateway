import type { ReactNode } from "react";

export function AuthFormPage({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section>
      <header className="mb-6 flex flex-col gap-2">
        <h1 className="text-3xl font-semibold tracking-tight">{title}</h1>
      </header>
      {children}
    </section>
  );
}
