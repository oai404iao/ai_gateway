import { useRef, type RefObject } from "react";
import { Outlet } from "react-router";

type DirectoryPosition = {
  listKey: string;
  modelId: string;
  search: string;
  scrollY: number;
};

export type ModelDirectoryContext = RefObject<DirectoryPosition | null>;

export function ModelWorkspaceLayout() {
  const position = useRef<DirectoryPosition | null>(null);
  return <Outlet context={position} />;
}
