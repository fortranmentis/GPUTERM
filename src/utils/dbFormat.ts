import type { DbKind, DbReading, DbStatus } from "../types/databases";

export function kindTitle(kind: DbKind): string {
  return kind === "postgres" ? "PostgreSQL" : "MySQL / MariaDB";
}

export function defaultPort(kind: DbKind): number {
  return kind === "postgres" ? 5432 : 3306;
}

/**
 * The per-second rate of the counter the kind reports best, from two
 * readings; null without a usable earlier one or after a server restart
 * (the counter went back).
 */
export function dbRate(
  kind: DbKind,
  previous: DbReading | null | undefined,
  current: DbStatus,
  at: number,
): number | null {
  const before = previous?.status;
  if (!previous || !before) return null;
  const seconds = (at - previous.at) / 1000;
  const counter = (status: DbStatus) =>
    kind === "postgres" ? status.commits : status.queries;
  const now = counter(current);
  const then = counter(before);
  if (!(seconds > 0) || now == null || then == null || now < then) return null;
  return (now - then) / seconds;
}

export function rateLabel(kind: DbKind): string {
  return kind === "postgres" ? "commits/s" : "queries/s";
}

/** Connections near the limit or a lagging replica need attention. */
export function needsAttention(status: DbStatus | null | undefined): boolean {
  if (!status) return false;
  const { connections, maxConnections } = status;
  if (connections != null && maxConnections != null && maxConnections > 0) {
    if (connections / maxConnections >= 0.9) return true;
  }
  return (status.replicationLagSecs ?? 0) >= 60;
}

/** `42/200 connections`, or `42 connections` without a known limit. */
export function connectionsLabel(status: DbStatus): string {
  const used = status.connections == null ? "?" : String(status.connections);
  return status.maxConnections == null
    ? `${used} connections`
    : `${used}/${status.maxConnections} connections`;
}

export function formatRate(rate: number): string {
  return rate.toFixed(1);
}

/** The card line for one database. */
export function dbCardLine(name: string, kind: DbKind, reading: DbReading | undefined): string {
  if (!reading) return `${name}: reading…`;
  if (!reading.status) return `${name}: ${reading.error ?? "unavailable"}`;
  let line = `${name}: ${connectionsLabel(reading.status)}`;
  if (reading.rate != null) line += `, ${formatRate(reading.rate)} ${rateLabel(kind)}`;
  return line;
}
