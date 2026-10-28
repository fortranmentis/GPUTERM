import { useEffect } from "react";
import { useSessionStore } from "../stores/sessionStore";
import {
  openWebService,
  useWebServicesStore,
  WEB_SERVICE_LABELS,
} from "../stores/webServicesStore";

/** One short remote command per host per interval. */
const WATCH_INTERVAL_MS = 20_000;

/**
 * Keeps the web-service list of every connected SSH host current and, unless
 * turned off, announces a newly started server with an "Open" toast. Ports
 * without a known web UI are listed but never announced.
 */
export function useWebServiceWatch() {
  const sessions = useSessionStore((state) => state.sessions);
  const connectedSessionIds = useSessionStore((state) => state.connectedSessionIds);
  const remoteIds = connectedSessionIds
    .filter((id) => sessions.some((session) => session.id === id && !session.isLocal))
    .join("|");

  useEffect(() => {
    const ids = remoteIds ? remoteIds.split("|") : [];
    const { scans, forget } = useWebServicesStore.getState();
    for (const id of Object.keys(scans)) {
      if (!ids.includes(id)) forget(id);
    }
    if (ids.length === 0) {
      return;
    }
    const scanAll = () => {
      for (const sessionId of ids) {
        void useWebServicesStore
          .getState()
          .refresh(sessionId)
          .then((added) => {
            const announced = added.filter((service) => service.kind !== "other");
            if (announced.length === 0 || !useWebServicesStore.getState().notify) {
              return;
            }
            const { sessions: profiles, setMessage } = useSessionStore.getState();
            const name = profiles.find((profile) => profile.id === sessionId)?.name ?? "a host";
            const service = announced[0];
            const more = announced.length > 1 ? ` and ${announced.length - 1} more` : "";
            setMessage({
              kind: "info",
              text: `${WEB_SERVICE_LABELS[service.kind]} started on ${name} (:${service.port})${more}`,
              action: {
                label: "Open",
                run: () =>
                  void openWebService(sessionId, service).catch((error) =>
                    setMessage({ kind: "error", text: String(error) }),
                  ),
              },
            });
          });
      }
    };
    scanAll();
    const timer = window.setInterval(scanAll, WATCH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [remoteIds]);
}
