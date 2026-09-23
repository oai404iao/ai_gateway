import { useSearchParams } from "react-router";

export function useListPagination() {
  const [params, setParams] = useSearchParams();
  const pageValue = Number(params.get("page") ?? 1);
  const sizeValue = Number(params.get("pageSize") ?? 20);
  const page = Number.isSafeInteger(pageValue) && pageValue > 0 ? pageValue : 1;
  const pageSize = [10, 20, 50].includes(sizeValue) ? sizeValue : 20;
  const update = (nextPage: number, nextSize: number) => {
    setParams((current) => {
      const next = new URLSearchParams(current);
      if (nextPage === 1) next.delete("page"); else next.set("page", String(nextPage));
      if (nextSize === 20) next.delete("pageSize"); else next.set("pageSize", String(nextSize));
      return next;
    }, { replace: true });
  };
  return {
    page, pageSize,
    onPageChange: (next: number) => update(next, pageSize),
    onPageSizeChange: (next: number) => update(1, next),
  };
}
