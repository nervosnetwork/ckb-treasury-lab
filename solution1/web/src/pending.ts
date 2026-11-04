/**
 * Pending vote submissions.
 *
 * A broadcast vote is kept here until the snapshot contains its vote cell. A
 * full page reload loses React state and CCC's in-memory cache, so this record
 * is what stops the page from offering a second submission while the first is
 * still pending.
 */

export interface PendingVote {
  network: string;
  proposalId: string;
  voterLock: string;
  txHash: string;
  direction: "yes" | "no";
  amount: string;
  createdAt: number;
}

export type PendingVotes = Record<string, PendingVote>;

export const PENDING_VOTES_KEY = "ckb-vote:pending-votes";

/** Stable key: one pending vote per network, proposal and voter lock. */
export function pendingVoteKey(network: string, proposalId: string, voterLock: string): string {
  return `${network}:${proposalId.toLowerCase()}:${voterLock.toLowerCase()}`;
}

function isPendingVote(value: unknown): value is PendingVote {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<PendingVote>;
  return (
    typeof record.network === "string" &&
    typeof record.proposalId === "string" &&
    typeof record.voterLock === "string" &&
    typeof record.txHash === "string" &&
    (record.direction === "yes" || record.direction === "no") &&
    typeof record.amount === "string" &&
    typeof record.createdAt === "number"
  );
}

export function loadPendingVotes(): PendingVotes {
  try {
    const raw = localStorage.getItem(PENDING_VOTES_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return {};
    const result: PendingVotes = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (isPendingVote(value)) {
        result[key] = value;
      }
    }
    return result;
  } catch {
    return {};
  }
}

export function savePendingVote(record: PendingVote): PendingVotes {
  const all = loadPendingVotes();
  all[pendingVoteKey(record.network, record.proposalId, record.voterLock)] = record;
  localStorage.setItem(PENDING_VOTES_KEY, JSON.stringify(all));
  return all;
}

export function clearPendingVotes(keys: string[]): PendingVotes {
  const all = loadPendingVotes();
  for (const key of keys) delete all[key];
  localStorage.setItem(PENDING_VOTES_KEY, JSON.stringify(all));
  return all;
}

/** Notifies the current page when another tab changes the pending records. */
export function subscribePendingVotes(listener: () => void): () => void {
  const handler = (event: StorageEvent) => {
    if (event.key === PENDING_VOTES_KEY) listener();
  };
  window.addEventListener("storage", handler);
  return () => window.removeEventListener("storage", handler);
}
