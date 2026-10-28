import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  BarChart3,
  BookOpen,
  Box,
  ChevronDown,
  ChevronRight,
  Code2,
  ExternalLink,
  FlaskConical,
  Globe,
  LineChart,
  LoaderCircle,
  Pin,
  PinOff,
  RefreshCw,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import { useSessionStore } from "../stores/sessionStore";
import {
  openWebService,
  useWebServicesStore,
  WEB_SERVICE_LABELS,
  webServiceKey,
} from "../stores/webServicesStore";
import type { ForwardSpec, SessionProfile } from "../types/session";
import type { WebService } from "../types/webServices";

const KIND_ICONS: Record<WebService["kind"], LucideIcon> = {
  tensorboard: LineChart,
  jupyter: BookOpen,
  mlflow: FlaskConical,
  ray: Activity,
  streamlit: BarChart3,
  gradio: Sparkles,
  "code-server": Code2,
  other: Globe,
};

/** The saved rule that already forwards a service, if any. */
function savedRuleFor(profile: SessionProfile, service: WebService) {
  return (profile.forwards ?? []).find(
    (rule) =>
      rule.kind === "local" && rule.destHost === service.host && rule.destPort === service.port,
  );
}

/**
 * Web UIs on the connected host — TensorBoard, Jupyter, MLflow and friends,
 * including ones in Docker containers — each opened in the browser through a
 * temporary SSH forward with one click, or pinned as a saved forward.
 * `useWebServiceWatch` keeps the list current; this only reads and refreshes.
 */
export function WebServices({ profile, connected }: { profile: SessionProfile; connected: boolean }) {
  const scan = useWebServicesStore((state) => state.scans[profile.id]);
  const scanError = useWebServicesStore((state) => state.errors[profile.id]);
  const scanning = useWebServicesStore((state) => state.scanning[profile.id] ?? false);
  const refresh = useWebServicesStore((state) => state.refresh);
  const notify = useWebServicesStore((state) => state.notify);
  const setNotify = useWebServicesStore((state) => state.setNotify);
  const setSessions = useSessionStore((state) => state.setSessions);
  const [open, setOpen] = useState(true);
  const [showOther, setShowOther] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setShowOther(false);
  }, [profile.id]);

  if (!connected) {
    return null;
  }

  const run = async (key: string, action: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await action();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(null);
    }
  };

  const pin = (service: WebService) =>
    run(`pin:${webServiceKey(service)}`, async () => {
      const saved = profile.forwards ?? [];
      const existing = savedRuleFor(profile, service);
      let forwards: ForwardSpec[];
      if (existing) {
        forwards = saved.filter((rule) => rule.id !== existing.id);
      } else {
        const spec = await invoke<ForwardSpec>("validate_forward", {
          spec: {
            id: "",
            kind: "local",
            bindHost: "127.0.0.1",
            // A privileged remote port cannot be reused here without root.
            bindPort: service.port >= 1024 ? service.port : service.port + 8000,
            destHost: service.host,
            destPort: service.port,
            enabled: true,
          },
        });
        forwards = [...saved, spec];
      }
      setSessions(
        await invoke<SessionProfile[]>("save_profile_forwards", {
          profileId: profile.id,
          forwards,
        }),
      );
    });

  const services = scan?.services ?? [];
  const known = services.filter((service) => service.kind !== "other");
  const other = services.filter((service) => service.kind === "other");
  const shownError = error ?? scanError;

  const row = (service: WebService) => {
    const key = webServiceKey(service);
    const Icon = KIND_ICONS[service.kind];
    const label =
      service.kind === "other" ? (service.detail ?? "Listening port") : WEB_SERVICE_LABELS[service.kind];
    const detail = service.kind === "other" ? null : service.detail;
    const rule = savedRuleFor(profile, service);
    return (
      <div className="port-forward-row web-service-row" key={key}>
        <Icon size={15} className={`web-service-icon ${service.kind}`} aria-hidden="true" />
        <span className="port-forward-main">
          <strong>
            <span className="web-service-name">{label}</span>
            <em>:{service.port}</em>
          </strong>
          {service.container && (
            <i className="web-service-container" title={`Docker container ${service.container}`}>
              <Box size={10} aria-hidden="true" />
              {service.container}
            </i>
          )}
          {detail && <small title={detail}>{detail}</small>}
        </span>
        <span className="port-forward-actions">
          <button
            className={`icon-button ghost${rule ? " active" : ""}`}
            type="button"
            aria-label={
              rule
                ? `Stop forwarding port ${service.port} on connect`
                : `Forward port ${service.port} on every connect`
            }
            title={rule ? "Forwarded on every connect — click to remove" : "Forward on every connect"}
            disabled={busy !== null}
            onClick={() => void pin(service)}
          >
            {rule ? <PinOff size={13} /> : <Pin size={13} />}
          </button>
          <button
            className="web-service-open"
            type="button"
            aria-label={`Open ${label} on port ${service.port}`}
            disabled={busy !== null}
            onClick={() => void run(key, () => openWebService(profile.id, service))}
          >
            {busy === key ? <LoaderCircle className="spin" size={12} /> : <ExternalLink size={12} />}
            Open
          </button>
        </span>
      </div>
    );
  };

  return (
    <section className="port-forwards web-services">
      <button
        className="port-forwards-toggle"
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Globe size={14} />
        <span>Web services</span>
        {scan?.supported && <small>{known.length} running</small>}
      </button>
      {open && (
        <div className="port-forwards-body">
          {shownError && (
            <div className="port-forward-error" role="alert">
              {shownError}
            </div>
          )}
          {scan?.supported && known.length === 0 && (
            <small className="port-forwards-empty">
              No TensorBoard, Jupyter or other web UI running. Links printed in the terminal also
              open with ⌘/Ctrl-click.
            </small>
          )}
          {!scan && !shownError && (
            <small className="port-forwards-empty">Looking for servers…</small>
          )}
          {known.map(row)}
          {other.length > 0 && (
            <>
              <button
                className="web-services-other-toggle"
                type="button"
                aria-expanded={showOther}
                onClick={() => setShowOther((value) => !value)}
              >
                {showOther ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                Other listening ports ({other.length})
              </button>
              {showOther && other.map(row)}
            </>
          )}
          {scan?.supported && (
            <div className="web-services-footer">
              <label className="web-services-notify">
                <input
                  type="checkbox"
                  checked={notify}
                  onChange={(event) => setNotify(event.target.checked)}
                />
                <span>Notify when one starts</span>
              </label>
              <button
                className="icon-button ghost"
                type="button"
                aria-label="Refresh web services"
                title="Refresh"
                disabled={scanning}
                onClick={() => void refresh(profile.id)}
              >
                <RefreshCw size={13} className={scanning ? "spin" : undefined} />
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
