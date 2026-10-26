/** "postgres" or "mysql" (MySQL and MariaDB). */
export type DbKind = "postgres" | "mysql";

/** `DbInstance` in ssh/db_monitor.rs. */
export type DbInstance = {
  id: string;
  name: string;
  kind: DbKind;
  /** The SSH profile the database is reached through. */
  profileId: string;
  /** As seen from the SSH server; empty means the local Unix socket. */
  host: string;
  port: number;
  user: string;
  /** PostgreSQL only; empty connects to the user's default database. */
  database: string;
  /** Whether a password is saved for it in the credential vault. */
  hasPassword: boolean;
};

/** What the form saves. `password`: undefined keeps the saved one, "" removes it. */
export type DbInstanceInput = {
  id?: string;
  name: string;
  kind: DbKind;
  profileId: string;
  host: string;
  port?: number;
  user: string;
  database: string;
  password?: string;
};

/** `DbStatus` in ssh/db_monitor.rs. Counters are totals since the server started. */
export type DbStatus = {
  instanceId: string;
  version: string | null;
  connections: number | null;
  maxConnections: number | null;
  active: number | null;
  idleInTransaction: number | null;
  waitingOnLocks: number | null;
  longestQuerySecs: number | null;
  commits: number | null;
  rollbacks: number | null;
  queries: number | null;
  slowQueries: number | null;
  cacheHitPercent: number | null;
  deadlocks: number | null;
  sizeBytes: number | null;
  uptimeSecs: number | null;
  replicationLagSecs: number | null;
  isReplica: boolean;
};

/** A status reading with the rate from the one before it. */
export type DbReading = {
  status: DbStatus | null;
  error: string | null;
  /** Milliseconds since the epoch. */
  at: number;
  /** Commits per second (PostgreSQL) or queries per second (MySQL). */
  rate: number | null;
};
