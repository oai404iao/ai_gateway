import { useCallback, useState } from "react";

export function useCursorPagination() {
  const [cursorStack, setCursorStack] = useState<(string | null)[]>([null]);

  const cursor = cursorStack[cursorStack.length - 1] ?? undefined;
  const page = cursorStack.length;
  const hasPrevious = cursorStack.length > 1;

  const goPrevious = useCallback(() => {
    setCursorStack((current) => (current.length > 1 ? current.slice(0, -1) : current));
  }, []);

  const goNext = useCallback((nextCursor: string | null | undefined) => {
    if (!nextCursor) return;
    setCursorStack((current) => [...current, nextCursor]);
  }, []);

  const reset = useCallback(() => {
    setCursorStack((current) => (current.length === 1 && current[0] === null ? current : [null]));
  }, []);

  return {
    cursor,
    page,
    hasPrevious,
    goPrevious,
    goNext,
    reset,
  };
}
