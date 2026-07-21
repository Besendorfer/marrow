import { useEffect, useMemo, useRef, useState } from "react";
import type { FileDiff, RiskLevel, ChangeGroup, HunkSignificanceFilter, SidebarView, ReviewThread } from "../types";
import { getFileName } from "../utils";

interface FileSidebarProps {
  files: FileDiff[];
  changeGroups: ChangeGroup[];
  selectedFile: FileDiff | null;
  onSelectFile: (file: FileDiff) => void;
  viewedFiles: Set<string>;
  staleViewedFiles: Set<string>;
  onToggleViewed: (filePath: string) => void;
  showHunkSignificance: boolean;
  hunkFilter: HunkSignificanceFilter;
  onHunkFilterChange: (filter: HunkSignificanceFilter) => void;
  sidebarView: SidebarView;
  onViewChange: (view: SidebarView) => void;
  commentThreads: ReviewThread[];
  selectedCommentFile: string | null;
  onSelectCommentFile: (path: string) => void;
  /** Emits the visible file paths in manifest order, for keyboard file navigation. */
  onVisibleFilesChange?: (paths: string[]) => void;
  onShowOverview?: () => void;
}

function getMaxHunkSignificance(file: FileDiff): string {
  const scores = file.hunk_scores ?? [];
  if (scores.length === 0) return "none";
  if (scores.includes("high")) return "high";
  if (scores.includes("medium")) return "medium";
  return "low";
}

function fileMatchesHunkFilter(file: FileDiff, filter: HunkSignificanceFilter): boolean {
  if (filter === "all") return true;
  const max = getMaxHunkSignificance(file);
  if (filter === "high") return max === "high";
  if (filter === "medium") return max === "high" || max === "medium";
  // "low" means show all (including low-only) — same as "all"
  return true;
}

function getDiffTypeIcon(diffType: string): string {
  switch (diffType) {
    case "added":
      return "A";
    case "removed":
      return "D";
    case "modified":
      return "M";
    default:
      return "?";
  }
}

function getDiffTypeClass(diffType: string): string {
  switch (diffType) {
    case "added":
      return "diff-type-added";
    case "removed":
      return "diff-type-removed";
    case "modified":
      return "diff-type-modified";
    default:
      return "";
  }
}

function sortByPathThenName(pathA: string, pathB: string): number {
  const dirA = pathA.substring(0, pathA.lastIndexOf("/"));
  const dirB = pathB.substring(0, pathB.lastIndexOf("/"));
  if (dirA !== dirB) return dirA.localeCompare(dirB);
  const nameA = pathA.substring(pathA.lastIndexOf("/") + 1);
  const nameB = pathB.substring(pathB.lastIndexOf("/") + 1);
  return nameA.localeCompare(nameB);
}

interface GroupedFiles {
  [category: string]: FileDiff[];
}

const NEEDS_ATTENTION = "Needs Attention";

const riskOrder: Record<RiskLevel, number> = {
  critical: 0,
  high: 1,
  medium: 2,
  low: 3,
};

/* ─── File Tree types & helpers ─────────────────────────────────────────── */

interface TreeNode {
  name: string;
  children: Map<string, TreeNode>;
  file: FileDiff | null;
}

function buildFileTree(files: FileDiff[]): TreeNode {
  const root: TreeNode = { name: "", children: new Map(), file: null };
  for (const file of files) {
    const parts = file.path.split("/");
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!node.children.has(parts[i])) {
        node.children.set(parts[i], {
          name: parts[i],
          children: new Map(),
          file: null,
        });
      }
      node = node.children.get(parts[i])!;
    }
    const fileName = parts[parts.length - 1];
    node.children.set(fileName, {
      name: fileName,
      children: new Map(),
      file,
    });
  }
  compressTree(root);
  return root;
}

function compressTree(node: TreeNode): void {
  const renames: [string, TreeNode][] = [];
  for (const [key, child] of node.children) {
    // Compress: if child is a dir with exactly one child that is also a dir, merge them
    while (
      child.file === null &&
      child.children.size === 1
    ) {
      const [, grandChild] = child.children.entries().next().value!;
      if (grandChild.file !== null) break; // don't merge into a file
      child.name = child.name + "/" + grandChild.name;
      child.children = grandChild.children;
    }
    if (child.name !== key) {
      renames.push([key, child]);
    }
    compressTree(child);
  }
  for (const [oldKey, child] of renames) {
    node.children.delete(oldKey);
    node.children.set(child.name, child);
  }
}

function sortedTreeEntries(node: TreeNode): TreeNode[] {
  const dirs: TreeNode[] = [];
  const fileNodes: TreeNode[] = [];
  for (const child of node.children.values()) {
    if (child.file) fileNodes.push(child);
    else dirs.push(child);
  }
  dirs.sort((a, b) => a.name.localeCompare(b.name));
  fileNodes.sort((a, b) => a.name.localeCompare(b.name));
  return [...dirs, ...fileNodes];
}

// Flatten the tree into the top-to-bottom order of *displayed* file rows,
// skipping the contents of collapsed folders. Mirrors TreeFolder's render so
// keyboard file nav follows exactly what's on screen.
function flattenTreePaths(node: TreeNode, depth: number, collapsed: Set<string>, out: string[]): void {
  for (const child of sortedTreeEntries(node)) {
    if (child.file) {
      out.push(child.file.path);
    } else if (!collapsed.has(`tree:${depth}:${child.name}`)) {
      flattenTreePaths(child, depth + 1, collapsed, out);
    }
  }
}

/* ─── Shared file item renderer ─────────────────────────────────────────── */

function FileItem({
  file,
  selectedFile,
  isViewed,
  isStale,
  onSelectFile,
  onToggleViewed,
  showPathHint,
}: {
  file: FileDiff;
  selectedFile: FileDiff | null;
  isViewed: boolean;
  isStale?: boolean;
  onSelectFile: (f: FileDiff) => void;
  onToggleViewed: (path: string) => void;
  showPathHint?: boolean;
}) {
  const isCritical =
    file.risk_level === "critical" || file.risk_level === "high";
  const notRelevant = file.classification === "NOT_RELEVANT";
  return (
    <div
      className={`file-item-wrapper ${isCritical ? "file-critical" : ""} ${notRelevant ? "file-not-relevant" : ""}`}
    >
      <button
        className={`file-item ${selectedFile?.path === file.path ? "selected" : ""} ${isViewed ? "viewed" : ""} ${isStale ? "stale" : ""}`}
        onClick={() => onSelectFile(file)}
        title={file.path}
      >
        <span
          className="viewed-checkbox"
          onClick={(e) => e.stopPropagation()}
        >
          <input
            type="checkbox"
            checked={isViewed}
            onChange={() => onToggleViewed(file.path)}
          />
        </span>
        {isStale && (
          <span className="stale-indicator" title="Changed since you last reviewed">&#x26A0;</span>
        )}
        <span
          className={`risk-dot risk-dot--${file.risk_level}`}
          title={`${file.risk_level} risk`}
        />
        {file.diff_type !== "modified" && (
          <span
            className={`diff-type-badge ${getDiffTypeClass(file.diff_type)}`}
          >
            {getDiffTypeIcon(file.diff_type)}
          </span>
        )}
        <span className="file-name">{getFileName(file.path)}</span>
        <span className="line-stats">
          <span className="line-stat-add">+{file.additions}</span>
          <span className="line-stat-del">-{file.deletions}</span>
        </span>
        {file.highlights?.length > 0 && (
          <span
            className="file-highlight-count"
            title={`${file.highlights.length} AI note${file.highlights.length === 1 ? "" : "s"}`}
          >
            {file.highlights.length}
          </span>
        )}
        {showPathHint && (
          <span className="file-path-hint">
            {file.path.split("/").slice(-2, -1)[0] || ""}
          </span>
        )}
      </button>
      <div className="file-summary">{file.reason}</div>
    </div>
  );
}

/* ─── Tree node renderer ────────────────────────────────────────────────── */

function TreeFolder({
  node,
  depth,
  collapsed,
  toggleCollapsed,
  selectedFile,
  viewedFiles,
  staleViewedFiles,
  onSelectFile,
  onToggleViewed,
}: {
  node: TreeNode;
  depth: number;
  collapsed: Set<string>;
  toggleCollapsed: (key: string) => void;
  selectedFile: FileDiff | null;
  viewedFiles: Set<string>;
  staleViewedFiles: Set<string>;
  onSelectFile: (f: FileDiff) => void;
  onToggleViewed: (path: string) => void;
}) {
  const children = sortedTreeEntries(node);
  return (
    <>
      {children.map((child) => {
        if (child.file) {
          return (
            <div key={child.file.path} style={{ paddingLeft: depth * 12 }}>
              <FileItem
                file={child.file}
                selectedFile={selectedFile}
                isViewed={viewedFiles.has(child.file.path)}
                isStale={staleViewedFiles.has(child.file.path)}
                onSelectFile={onSelectFile}
                onToggleViewed={onToggleViewed}
              />
            </div>
          );
        }
        const folderKey = `tree:${depth}:${child.name}`;
        const isCollapsed = collapsed.has(folderKey);
        return (
          <div key={folderKey} className="tree-folder">
            <button
              className="tree-folder-toggle"
              style={{ paddingLeft: 16 + depth * 12 }}
              onClick={() => toggleCollapsed(folderKey)}
            >
              <span
                className={`collapse-chevron ${isCollapsed ? "collapsed" : ""}`}
              >
                &#9662;
              </span>
              <span className="tree-folder-icon">&#128193;</span>
              <span className="tree-folder-name">{child.name}</span>
            </button>
            {!isCollapsed && (
              <TreeFolder
                node={child}
                depth={depth + 1}
                collapsed={collapsed}
                toggleCollapsed={toggleCollapsed}
                selectedFile={selectedFile}
                viewedFiles={viewedFiles}
                staleViewedFiles={staleViewedFiles}
                onSelectFile={onSelectFile}
                onToggleViewed={onToggleViewed}
              />
            )}
          </div>
        );
      })}
    </>
  );
}

/* ─── Comment file list ──────────────────────────────────────────────────── */

function CommentFileList({
  threads,
  selectedFile,
  onSelectFile,
}: {
  threads: ReviewThread[];
  selectedFile: string | null;
  onSelectFile: (path: string) => void;
}) {
  const fileMap = useMemo(() => {
    const map = new Map<string, { total: number; unresolved: number }>();
    for (const t of threads) {
      const entry = map.get(t.path) ?? { total: 0, unresolved: 0 };
      entry.total++;
      if (!t.is_resolved) entry.unresolved++;
      map.set(t.path, entry);
    }
    // Sort: files with unresolved threads first, then alphabetical
    const sorted = [...map.entries()].sort((a, b) => {
      if (a[1].unresolved > 0 && b[1].unresolved === 0) return -1;
      if (a[1].unresolved === 0 && b[1].unresolved > 0) return 1;
      return a[0].localeCompare(b[0]);
    });
    return sorted;
  }, [threads]);

  if (threads.length === 0) {
    return (
      <div className="comment-file-list-empty">
        No review threads on this PR
      </div>
    );
  }

  return (
    <div className="comment-file-list">
      {fileMap.map(([path, counts]) => {
        const fileName = getFileName(path);
        const dirHint = path.split("/").slice(-2, -1)[0] || "";
        return (
          <button
            key={path}
            className={`comment-file-item ${selectedFile === path ? "selected" : ""}`}
            onClick={() => onSelectFile(path)}
            title={path}
          >
            <span className="file-name">{fileName}</span>
            <span className="file-path-hint">{dirHint}</span>
            {counts.unresolved > 0 ? (
              <span className="unresolved-badge">
                {counts.unresolved}
              </span>
            ) : (
              <span className="comment-count-badge">
                {counts.total}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/* ─── Main component ────────────────────────────────────────────────────── */

export function FileSidebar({
  files,
  changeGroups,
  selectedFile,
  onSelectFile,
  viewedFiles,
  staleViewedFiles,
  onToggleViewed,
  showHunkSignificance,
  hunkFilter,
  onHunkFilterChange,
  sidebarView: view,
  onViewChange: setView,
  commentThreads,
  selectedCommentFile,
  onSelectCommentFile,
  onVisibleFilesChange,
  onShowOverview,
}: FileSidebarProps) {
  const hasGroups = changeGroups.length > 0;
  const prevHasGroups = useRef(hasGroups);

  useEffect(() => {
    if (hasGroups && !prevHasGroups.current) {
      setView("groups");
    }
    prevHasGroups.current = hasGroups;
  }, [hasGroups]);

  const criticalFiles = useMemo(() => {
    return files
      .filter((f) => f.risk_level === "critical" || f.risk_level === "high")
      .sort((a, b) => {
        const riskDiff =
          (riskOrder[a.risk_level] ?? 2) - (riskOrder[b.risk_level] ?? 2);
        if (riskDiff !== 0) return riskDiff;
        return sortByPathThenName(a.path, b.path);
      });
  }, [files]);

  const grouped = useMemo(() => {
    const groups: GroupedFiles = {};
    for (const file of files) {
      const cat = file.category || "Other";
      if (!groups[cat]) groups[cat] = [];
      groups[cat].push(file);
    }
    for (const cat of Object.keys(groups)) {
      groups[cat].sort(
        (a, b) =>
          (riskOrder[a.risk_level] ?? 2) - (riskOrder[b.risk_level] ?? 2)
      );
    }
    return groups;
  }, [files]);

  const filesByPath = useMemo(() => {
    const map = new Map<string, FileDiff>();
    for (const f of files) map.set(f.path, f);
    return map;
  }, [files]);

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set([NEEDS_ATTENTION]));
  const toggleCollapsed = (section: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(section)) next.delete(section);
      else next.add(section);
      return next;
    });
  };

  const [hideViewed, setHideViewed] = useState(true);
  // NOT_RELEVANT files (tests, generated, presentational — judged noise by the
  // AI) are hidden by default; this toggle reveals them on demand.
  const [hideNotRelevant, setHideNotRelevant] = useState(true);

  const notRelevantCount = useMemo(
    () => files.filter((f) => f.classification === "NOT_RELEVANT").length,
    [files],
  );

  const visiblePaths = useMemo(() => {
    const needsViewedFilter = hideViewed;
    const needsRelevanceFilter = hideNotRelevant && notRelevantCount > 0;
    const needsHunkFilter = showHunkSignificance && hunkFilter !== "all";
    if (!needsViewedFilter && !needsRelevanceFilter && !needsHunkFilter) return null; // null = show all
    const set = new Set<string>();
    for (const f of files) {
      if (needsViewedFilter && viewedFiles.has(f.path)) continue;
      if (needsRelevanceFilter && f.classification === "NOT_RELEVANT") continue;
      if (needsHunkFilter && !fileMatchesHunkFilter(f, hunkFilter)) continue;
      set.add(f.path);
    }
    return set;
  }, [files, viewedFiles, hideViewed, hideNotRelevant, notRelevantCount, showHunkSignificance, hunkFilter]);

  const isVisible = (f: { path: string }) => visiblePaths === null || visiblePaths.has(f.path);

  const visibleCriticalFiles = useMemo(() => {
    if (!visiblePaths) return criticalFiles;
    return criticalFiles.filter((f) => visiblePaths.has(f.path));
  }, [criticalFiles, visiblePaths]);

  const visibleGrouped = useMemo(() => {
    if (!visiblePaths) return grouped;
    const filtered: GroupedFiles = {};
    for (const [cat, catFiles] of Object.entries(grouped)) {
      const visible = catFiles.filter((f) => visiblePaths.has(f.path));
      if (visible.length > 0) filtered[cat] = visible;
    }
    return filtered;
  }, [grouped, visiblePaths]);

  const visibleFileTree = useMemo(() => {
    if (!visiblePaths) return buildFileTree(files);
    return buildFileTree(files.filter((f) => visiblePaths.has(f.path)));
  }, [files, visiblePaths]);

  // Files not in any AI change group (NOT_RELEVANT files aren't grouped, and the
  // grouping step can omit a file) — surfaced under "Other" in the Groups view
  // so nothing silently disappears there.
  const ungroupedFiles = useMemo(() => {
    if (!hasGroups) return [];
    const groupedPaths = new Set(changeGroups.flatMap((g) => g.file_paths));
    return files.filter(
      (f) => !groupedPaths.has(f.path) && (visiblePaths === null || visiblePaths.has(f.path)),
    );
  }, [files, changeGroups, hasGroups, visiblePaths]);

  const visibleCategories = useMemo(() => {
    const order = ["Business Logic", "Infrastructure", "Domain Types", "Other"];
    return Object.keys(visibleGrouped).sort((a, b) => {
      const ai = order.indexOf(a);
      const bi = order.indexOf(b);
      return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
    });
  }, [visibleGrouped]);

  // The flat top-to-bottom order of file rows as actually displayed for the
  // current view — respecting filters and collapsed sections, deduped to first
  // occurrence (critical files also appear under their category). Drives the
  // [ / ] navigation shortcuts so they follow the visible list exactly.
  const navigableOrder = useMemo(() => {
    const out: string[] = [];
    const seen = new Set<string>();
    const push = (path: string) => {
      if (!seen.has(path)) { seen.add(path); out.push(path); }
    };
    if (view === "groups") {
      changeGroups.forEach((group, idx) => {
        if (collapsed.has(`group:${idx}`)) return;
        for (const p of group.file_paths) {
          const f = filesByPath.get(p);
          if (f && isVisible(f)) push(f.path);
        }
      });
      if (!collapsed.has("group:other")) ungroupedFiles.forEach((f) => push(f.path));
    } else if (view === "category") {
      if (!collapsed.has(NEEDS_ATTENTION)) visibleCriticalFiles.forEach((f) => push(f.path));
      for (const category of visibleCategories) {
        if (collapsed.has(category)) continue;
        visibleGrouped[category].forEach((f) => push(f.path));
      }
    } else if (view === "tree") {
      flattenTreePaths(visibleFileTree, 0, collapsed, out);
    }
    // "comments" view drives a separate selection model — emit nothing.
    return out;
  }, [view, changeGroups, collapsed, filesByPath, visiblePaths, ungroupedFiles, visibleCriticalFiles, visibleCategories, visibleGrouped, visibleFileTree]);

  useEffect(() => {
    onVisibleFilesChange?.(navigableOrder);
  }, [navigableOrder, onVisibleFilesChange]);

  const viewedCount = viewedFiles.size;
  const staleCount = staleViewedFiles.size;
  const totalCount = files.length;

  return (
    <aside className="file-sidebar">
      {onShowOverview && view !== "comments" && (
        <button
          className={`sidebar-overview-link${selectedFile === null ? " active" : ""}`}
          onClick={onShowOverview}
          title="Back to the PR summary and change groups"
        >
          ← Overview
        </button>
      )}
      <div className="sidebar-header">
        <div className="sidebar-header-top">
          <span className="sidebar-title">Files</span>
          <div className="sidebar-view-toggle">
            {hasGroups && (
              <button
                className={view === "groups" ? "active" : ""}
                onClick={() => setView("groups")}
                title="Group by logical change"
              >
                Groups
              </button>
            )}
            <button
              className={view === "comments" ? "active" : ""}
              onClick={() => setView("comments")}
              title="Review comments"
            >
              Comments
            </button>
            <button
              className={view === "category" ? "active" : ""}
              onClick={() => setView("category")}
              title="Group by category"
            >
              Category
            </button>
            <button
              className={view === "tree" ? "active" : ""}
              onClick={() => setView("tree")}
              title="File tree"
            >
              Tree
            </button>
          </div>
        </div>
        <span className="sidebar-file-count">
          {viewedCount}/{totalCount} viewed
          {staleCount > 0 && (
            <span className="stale-count" title={`${staleCount} file(s) changed since reviewed`}>
              {" "}({staleCount} changed)
            </span>
          )}
        </span>
      </div>
      {viewedCount > 0 && (
        <button
          className={`hide-viewed-toggle ${hideViewed ? "active" : ""}`}
          onClick={() => setHideViewed((h) => !h)}
        >
          {hideViewed ? "Show reviewed" : "Hide reviewed"}
          <span className="hide-viewed-count">{viewedCount}</span>
        </button>
      )}
      {notRelevantCount > 0 && (
        <button
          className={`hide-viewed-toggle ${hideNotRelevant ? "active" : ""}`}
          onClick={() => setHideNotRelevant((h) => !h)}
          title="Files the AI judged not relevant to review"
        >
          {hideNotRelevant ? "Show not relevant" : "Hide not relevant"}
          <span className="hide-viewed-count">{notRelevantCount}</span>
        </button>
      )}
      {showHunkSignificance && (
        <div className="sidebar-filter-bar">
          <span className="sidebar-filter-label">Show hunks:</span>
          <div className="sidebar-filter-toggle">
            {(["all", "high", "medium"] as HunkSignificanceFilter[]).map((level) => (
              <button
                key={level}
                className={hunkFilter === level ? "active" : ""}
                onClick={() => onHunkFilterChange(level)}
              >
                {level === "all" ? "All" : level === "high" ? "High" : "Med+"}
              </button>
            ))}
          </div>
        </div>
      )}
      <nav className="file-list">
        {view === "comments" ? (
          <CommentFileList
            threads={commentThreads}
            selectedFile={selectedCommentFile}
            onSelectFile={onSelectCommentFile}
          />
        ) : view === "groups" ? (
          <>
            {changeGroups.map((group, idx) => {
              const groupKey = `group:${idx}`;
              const groupFiles = group.file_paths
                .map((p) => filesByPath.get(p))
                .filter((f): f is FileDiff => f !== undefined && isVisible(f));
              if (groupFiles.length === 0) return null;
              return (
                <div key={groupKey} className="file-group change-group">
                  <button
                    className="group-header group-toggle"
                    onClick={() => toggleCollapsed(groupKey)}
                  >
                    <span className={`collapse-chevron ${collapsed.has(groupKey) ? "collapsed" : ""}`}>&#9662;</span>
                    {group.label}
                    <span className="group-count">{groupFiles.length}</span>
                  </button>
                  {!collapsed.has(groupKey) && (<>
                  <div className="change-group-description">{group.description}</div>
                  {groupFiles.map((file) => (
                    <FileItem
                      key={file.path}
                      file={file}
                      selectedFile={selectedFile}
                      isViewed={viewedFiles.has(file.path)}
                      isStale={staleViewedFiles.has(file.path)}
                      onSelectFile={onSelectFile}
                      onToggleViewed={onToggleViewed}
                      showPathHint
                    />
                  ))}
                  </>)}
                </div>
              );
            })}
            {ungroupedFiles.length > 0 && (() => {
              const groupKey = "group:other";
              return (
                <div className="file-group change-group">
                  <button
                    className="group-header group-toggle"
                    onClick={() => toggleCollapsed(groupKey)}
                  >
                    <span className={`collapse-chevron ${collapsed.has(groupKey) ? "collapsed" : ""}`}>&#9662;</span>
                    Other
                    <span className="group-count">{ungroupedFiles.length}</span>
                  </button>
                  {!collapsed.has(groupKey) && (<>
                  <div className="change-group-description">Files not part of a change group</div>
                  {ungroupedFiles.map((file) => (
                    <FileItem
                      key={file.path}
                      file={file}
                      selectedFile={selectedFile}
                      isViewed={viewedFiles.has(file.path)}
                      isStale={staleViewedFiles.has(file.path)}
                      onSelectFile={onSelectFile}
                      onToggleViewed={onToggleViewed}
                      showPathHint
                    />
                  ))}
                  </>)}
                </div>
              );
            })()}
          </>
        ) : view === "category" ? (
          <>
            {visibleCriticalFiles.length > 0 && (
              <div className="file-group critical-group">
                <button
                  className="group-header group-toggle"
                  onClick={() => toggleCollapsed(NEEDS_ATTENTION)}
                >
                  <span className={`collapse-chevron ${collapsed.has(NEEDS_ATTENTION) ? "collapsed" : ""}`}>&#9662;</span>
                  {NEEDS_ATTENTION}
                  <span className="group-count">{visibleCriticalFiles.length}</span>
                </button>
                {!collapsed.has(NEEDS_ATTENTION) && visibleCriticalFiles.map((file) => (
                  <FileItem
                    key={file.path}
                    file={file}
                    selectedFile={selectedFile}
                    isViewed={viewedFiles.has(file.path)}
                    isStale={staleViewedFiles.has(file.path)}
                    onSelectFile={onSelectFile}
                    onToggleViewed={onToggleViewed}
                    showPathHint
                  />
                ))}
              </div>
            )}
            {visibleCategories.map((category) => (
              <div key={category} className="file-group">
                <button
                  className="group-header group-toggle"
                  onClick={() => toggleCollapsed(category)}
                >
                  <span className={`collapse-chevron ${collapsed.has(category) ? "collapsed" : ""}`}>&#9662;</span>
                  {category}
                  <span className="group-count">{visibleGrouped[category].length}</span>
                </button>
                {!collapsed.has(category) && visibleGrouped[category].map((file) => (
                  <FileItem
                    key={file.path}
                    file={file}
                    selectedFile={selectedFile}
                    isViewed={viewedFiles.has(file.path)}
                    isStale={staleViewedFiles.has(file.path)}
                    onSelectFile={onSelectFile}
                    onToggleViewed={onToggleViewed}
                    showPathHint
                  />
                ))}
              </div>
            ))}
          </>
        ) : (
          <div className="file-tree">
            <TreeFolder
              node={visibleFileTree}
              depth={0}
              collapsed={collapsed}
              toggleCollapsed={toggleCollapsed}
              selectedFile={selectedFile}
              viewedFiles={viewedFiles}
              staleViewedFiles={staleViewedFiles}
              onSelectFile={onSelectFile}
              onToggleViewed={onToggleViewed}
            />
          </div>
        )}
      </nav>
    </aside>
  );
}
