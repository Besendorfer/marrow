import type { Job } from "../api";

export function statusLabel(job: Job): string {
  switch (job.status) {
    case "Queued":
      return "Waiting";
    case "InProgress":
      return "Running";
    case "Succeeded":
      return "Done";
    case "Failed":
      return "Failed";
    default:
      return "Unknown";
  }
}
