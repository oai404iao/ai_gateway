import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";

interface EmptyStateProps {
  title: string;
  description?: string;
  className?: string;
}

/** Composed empty state used by list/log views. */
export function EmptyState({ title, description, className }: EmptyStateProps) {
  return (
    <Empty className={className}>
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        {description ? <EmptyDescription>{description}</EmptyDescription> : null}
      </EmptyHeader>
    </Empty>
  );
}
