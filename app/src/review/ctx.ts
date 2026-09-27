// The per-render review context: all state (useReviewState) plus every
// handler from the review/ modules, flattened (issue #238 phase 2).

import type { ReviewState } from "./useReviewState";
import type { TabsApi } from "./tabs";
import type { NavigationApi } from "./navigation";
import type { LoadingApi } from "./loading";
import type { ProgressApi } from "./progress";
import type { ChecksApi } from "./checks";
import type { ChatApi } from "./chat";
import type { CommentsApi } from "./comments";
import type { CommitsApi } from "./commits";

// An interface (not an intersection alias) so the factories can take it as a
// parameter while it is built from their own return types.
export interface ReviewCtx extends
  ReviewState,
  TabsApi,
  NavigationApi,
  LoadingApi,
  ProgressApi,
  ChecksApi,
  ChatApi,
  CommentsApi,
  CommitsApi {}
