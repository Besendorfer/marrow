import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { FileDiff, SearchMatch } from "../types";
import { getFileName } from "../utils";

type SearchMode = "local" | "global";

/** Imperative handle so a global shortcut (e.g. "/") can open the search bar. */
export interface SearchBarHandle {
  open: (mode?: SearchMode) => void;
}

interface SearchBarProps {
  files: FileDiff[];
  selectedFile: FileDiff | null;
  onSelectFile: (file: FileDiff) => void;
  onHighlightMatches: (matches: SearchMatch[], currentIndex: number, query: string) => void;
  onClearHighlights: () => void;
  /** Notified whenever the bar opens/closes so the parent can track overlay state. */
  onOpenChange?: (open: boolean) => void;
}

function searchInContent(
  content: string,
  query: string,
  filePath: string,
): SearchMatch[] {
  if (!content || !query) return [];
  const lower = query.toLowerCase();
  const lines = content.split("\n");
  const matches: SearchMatch[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const lineLower = line.toLowerCase();
    let start = 0;
    while (true) {
      const idx = lineLower.indexOf(lower, start);
      if (idx === -1) break;
      matches.push({
        filePath,
        lineNumber: i + 1,
        lineContent: line,
        matchStart: idx,
        matchLength: query.length,
      });
      start = idx + 1;
    }
  }
  return matches;
}

function dedupeMatches(matches: SearchMatch[]): SearchMatch[] {
  const seen = new Set<string>();
  return matches.filter((m) => {
    const key = `${m.filePath}:${m.lineNumber}:${m.matchStart}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export const SearchBar = forwardRef<SearchBarHandle, SearchBarProps>(function SearchBar({
  files,
  selectedFile,
  onSelectFile,
  onHighlightMatches,
  onClearHighlights,
  onOpenChange,
}: SearchBarProps, ref) {
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<SearchMode>("local");
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [currentIndex, setCurrentIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);

  useImperativeHandle(ref, () => ({
    open: (m: SearchMode = "local") => {
      setMode(m);
      setOpen(true);
      setCurrentIndex(0);
      setTimeout(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      }, 0);
    },
  }), []);

  // Report open/close transitions to the parent (for overlay-aware shortcuts).
  useEffect(() => {
    onOpenChange?.(open);
  }, [open, onOpenChange]);

  // Debounce the query for global search (local stays instant)
  useEffect(() => {
    if (mode === "local") {
      setDebouncedQuery(query);
      return;
    }
    const timer = setTimeout(() => setDebouncedQuery(query), 150);
    return () => clearTimeout(timer);
  }, [query, mode]);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setDebouncedQuery("");
    setCurrentIndex(0);
    onClearHighlights();
  }, [onClearHighlights]);

  // Keyboard shortcuts
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Cmd+F = local find, Cmd+Shift+F = global find
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        const newMode = e.shiftKey ? "global" : "local";
        if (open && mode === newMode) {
          // Already open in the same mode — just focus
          inputRef.current?.focus();
          inputRef.current?.select();
        } else {
          setMode(newMode);
          setOpen(true);
          setCurrentIndex(0);
          // If switching modes, keep the query
        }
        setTimeout(() => {
          inputRef.current?.focus();
          inputRef.current?.select();
        }, 0);
      }
      if (e.key === "Escape" && open) {
        e.preventDefault();
        close();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [open, mode, close]);

  // Compute matches — local uses instant query, global uses debounced
  const effectiveQuery = mode === "local" ? query : debouncedQuery;
  const allMatches = useMemo(() => {
    if (!effectiveQuery) return [];
    if (mode === "global" && effectiveQuery.length < 2) return [];
    if (mode === "local") {
      if (!selectedFile) return [];
      return dedupeMatches([
        ...searchInContent(selectedFile.head_content, effectiveQuery, selectedFile.path),
        ...searchInContent(selectedFile.base_content, effectiveQuery, selectedFile.path),
      ]);
    } else {
      const MAX_GLOBAL_MATCHES = 1000;
      const matches: SearchMatch[] = [];
      for (const file of files) {
        matches.push(...dedupeMatches([
          ...searchInContent(file.head_content, effectiveQuery, file.path),
          ...searchInContent(file.base_content, effectiveQuery, file.path),
        ]));
        if (matches.length >= MAX_GLOBAL_MATCHES) {
          return matches.slice(0, MAX_GLOBAL_MATCHES);
        }
      }
      return matches;
    }
  }, [effectiveQuery, mode, selectedFile, files]);

  // Group by file for global results
  const groupedResults = useMemo(() => {
    if (mode !== "global") return null;
    const groups = new Map<string, SearchMatch[]>();
    for (const m of allMatches) {
      const arr = groups.get(m.filePath);
      if (arr) arr.push(m);
      else groups.set(m.filePath, [m]);
    }
    return groups;
  }, [allMatches, mode]);

  // Clamp current index
  useEffect(() => {
    if (currentIndex >= allMatches.length) {
      setCurrentIndex(Math.max(0, allMatches.length - 1));
    }
  }, [allMatches.length, currentIndex]);

  // Notify parent of highlights
  useEffect(() => {
    if (!open || allMatches.length === 0) {
      onClearHighlights();
      return;
    }
    onHighlightMatches(allMatches, currentIndex, effectiveQuery);
  }, [allMatches, currentIndex, open]);

  function goNext() {
    if (allMatches.length === 0) return;
    const next = (currentIndex + 1) % allMatches.length;
    setCurrentIndex(next);
    navigateToMatch(next);
  }

  function goPrev() {
    if (allMatches.length === 0) return;
    const prev = (currentIndex - 1 + allMatches.length) % allMatches.length;
    setCurrentIndex(prev);
    navigateToMatch(prev);
  }

  function navigateToMatch(index: number) {
    const match = allMatches[index];
    if (!match) return;
    // If global mode, switch to the matched file
    if (mode === "global") {
      const file = files.find((f) => f.path === match.filePath);
      if (file && file.path !== selectedFile?.path) {
        onSelectFile(file);
      }
    }
  }

  function handleResultClick(match: SearchMatch, index: number) {
    setCurrentIndex(index);
    const file = files.find((f) => f.path === match.filePath);
    if (file) {
      onSelectFile(file);
    }
    onHighlightMatches(allMatches, index, effectiveQuery);
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) goPrev();
      else goNext();
    }
  }

  // Scroll active result into view
  useEffect(() => {
    if (mode !== "global" || !resultsRef.current) return;
    const active = resultsRef.current.querySelector(".search-result-item.active");
    if (active) {
      active.scrollIntoView({ block: "nearest" });
    }
  }, [currentIndex, mode]);

  if (!open) return null;

  // Compute a flat index for global results to map back to allMatches index
  let globalFlatIndex = 0;

  return (
    <div className={`search-bar ${mode}`}>
      <div className="search-bar-input-row">
        <div className="search-mode-toggle">
          <button
            className={mode === "local" ? "active" : ""}
            onClick={() => { setMode("local"); setCurrentIndex(0); }}
            title="Find in current file (Cmd+F)"
          >
            File
          </button>
          <button
            className={mode === "global" ? "active" : ""}
            onClick={() => { setMode("global"); setCurrentIndex(0); }}
            title="Find in all files (Cmd+Shift+F)"
          >
            All Files
          </button>
        </div>
        <input
          ref={inputRef}
          className="search-input"
          type="text"
          value={query}
          onChange={(e) => { setQuery(e.target.value); setCurrentIndex(0); }}
          onKeyDown={handleKeyDown}
          placeholder={mode === "local" ? "Find in file..." : "Find in all files..."}
          autoFocus
        />
        <span className="search-count">
          {query
            ? allMatches.length > 0
              ? `${currentIndex + 1}/${allMatches.length}`
              : "No results"
            : ""}
        </span>
        <button className="search-nav-btn" onClick={goPrev} disabled={allMatches.length === 0} title="Previous (Shift+Enter)">
          &#9650;
        </button>
        <button className="search-nav-btn" onClick={goNext} disabled={allMatches.length === 0} title="Next (Enter)">
          &#9660;
        </button>
        <button className="search-close-btn" onClick={close} title="Close (Esc)" aria-label="Close search">
          &times;
        </button>
      </div>
      {mode === "global" && query && groupedResults && (
        <div className="search-results-panel" ref={resultsRef}>
          {groupedResults.size === 0 ? (
            <div className="search-no-results">No matches found</div>
          ) : (
            Array.from(groupedResults.entries()).map(([filePath, matches]) => {
              const startIdx = globalFlatIndex;
              globalFlatIndex += matches.length;
              return (
                <div key={filePath} className="search-result-group">
                  <div className="search-result-file">
                    <span className="search-result-filename">{getFileName(filePath)}</span>
                    <span className="search-result-filepath">{filePath}</span>
                    <span className="search-result-file-count">{matches.length}</span>
                  </div>
                  {matches.map((match, i) => {
                    const flatIdx = startIdx + i;
                    const isActive = flatIdx === currentIndex;
                    // Build highlighted line preview
                    const before = match.lineContent.slice(0, match.matchStart);
                    const matched = match.lineContent.slice(match.matchStart, match.matchStart + match.matchLength);
                    const after = match.lineContent.slice(match.matchStart + match.matchLength);
                    return (
                      <button
                        key={`${match.lineNumber}-${match.matchStart}`}
                        className={`search-result-item ${isActive ? "active" : ""}`}
                        onClick={() => handleResultClick(match, flatIdx)}
                      >
                        <span className="search-result-line-num">L{match.lineNumber}</span>
                        <span className="search-result-line-text">
                          <span>{truncateLeft(before, 40)}</span>
                          <mark>{matched}</mark>
                          <span>{truncateRight(after, 60)}</span>
                        </span>
                      </button>
                    );
                  })}
                </div>
              );
            })
          )}
        </div>
      )}
    </div>
  );
});

function truncateLeft(s: string, maxLen: number): string {
  if (s.length <= maxLen) return s;
  return "..." + s.slice(s.length - maxLen);
}

function truncateRight(s: string, maxLen: number): string {
  if (s.length <= maxLen) return s;
  return s.slice(0, maxLen) + "...";
}
