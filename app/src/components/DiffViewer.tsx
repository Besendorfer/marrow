import {
  Accessor,
  Component,
  For,
  Setter,
  Show,
  createSignal,
} from "solid-js";
import Hunks from "./Hunks";
import { Hunk, Position } from "../bindings";
import FileIcon from "./FileIcon";
import { DIFF_SIDE_WIDTH, MODIFIED_COLOR } from "../consts";
import DiffSide from "./DiffSide";
import { isHunkCollapsed, setHunkCollapsed } from "../utils/hunks";

const DiffViewer: Component<{
  path: string;
  hunks: Hunk[];
  isUnified: Accessor<boolean>;
  setIsUnified: Setter<boolean>;
  hunksLength: number;
  comments: Record<string, Position[]>;
  toggleAllHunks: (state: boolean) => void;
}> = (props) => {
  const [allExpanded, setAllExpanded] = createSignal(false);
  const expandAllHunks = () => {
    props.toggleAllHunks(true);
    setAllExpanded(true);
  };
  const collapseAllHunks = () => {
    props.toggleAllHunks(false);
    setAllExpanded(false);
  };

  return (
    <div
      class="rounded-md border-2 border-transparent"
      style={{ "border-color": MODIFIED_COLOR }}
    >
      <div class="flex select-none justify-between rounded-t-md border-b-2 border-b-transparent bg-neutral-800 p-2">
        <div class="flex items-center gap-2">
          <FileIcon path={props.path} /> {props.path}
        </div>
        <div class="flex items-center gap-2">
          <Show
            when={allExpanded()}
            fallback={
              <button class="text-sm" onClick={() => expandAllHunks()}>
                Expand All ({props.hunksLength})
              </button>
            }
          >
            <button class="text-sm" onClick={() => collapseAllHunks()}>
              Collapse All
            </button>
          </Show>
          <div class="flex items-center gap-1">
            <input
              class="w-4"
              type="checkbox"
              name="unified"
              id="unified"
              checked={props.isUnified()}
              onChange={() => props.setIsUnified(!props.isUnified())}
            />
            <label for="unified" class="text-sm">
              Unified
            </label>
          </div>
        </div>
      </div>
      <Show when={!props.isUnified()}>
        <div class="flex">
          <div style={{ width: DIFF_SIDE_WIDTH }}>
            <For each={props.hunks}>
              {(hunk) => (
                <DiffSide
                  comments={props.comments[props.path]}
                  side="left"
                  hunk={hunk}
                  filePath={props.path}
                  isHunkCollapsed={() =>
                    isHunkCollapsed(props.path, hunk.header)
                  }
                  toggleHunkCollapsed={() =>
                    setHunkCollapsed(
                      props.path,
                      hunk.header,
                      !isHunkCollapsed(props.path, hunk.header)
                    )
                  }
                />
              )}
            </For>
          </div>
          <div class="border-l-2 border-l-transparent" style={{ "border-color": MODIFIED_COLOR }}>
            <For each={props.hunks}>
              {(hunk) => (
                <DiffSide
                  comments={props.comments[props.path]}
                  side="right"
                  hunk={hunk}
                  filePath={props.path}
                  isHunkCollapsed={() =>
                    isHunkCollapsed(props.path, hunk.header)
                  }
                  toggleHunkCollapsed={() =>
                    setHunkCollapsed(
                      props.path,
                      hunk.header,
                      !isHunkCollapsed(props.path, hunk.header)
                    )
                  }
                />
              )}
            </For>
          </div>
        </div>
      </Show>

      <Show when={props.isUnified()}>
        <For each={props.hunks}>
          {(hunk) => (
            <Hunks
              comments={props.comments[props.path]}
              hunk={hunk}
              path={props.path}
              isHunkCollapsed={() => isHunkCollapsed(props.path, hunk.header)}
              toggleHunkCollapsed={() =>
                setHunkCollapsed(
                  props.path,
                  hunk.header,
                  !isHunkCollapsed(props.path, hunk.header)
                )
              }
            />
          )}
        </For>
      </Show>
    </div>
  );
};

export default DiffViewer;
