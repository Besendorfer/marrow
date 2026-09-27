export interface Job {
  id: number;
  status: "Queued" | "InProgress" | "Succeeded" | "Failed";
}

export async function fetchJob(id: number): Promise<Job> {
  const res = await fetch(`/jobs/${id}`);
  return res.json();
}
