import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  Globe,
  LineChart,
  LoaderCircle,
  RefreshCw,
} from "lucide-react";
import type { SessionProfile } from "../types/session";
import type { WebService, WebServiceScan } from "../types/webServices";

/** Each scan is one short command on the pooled connection. */
const SCAN_INTERVAL_MS = 15_000;

const KIND_LABELS: Record<WebService["kind"], string> = {
  tensorboard: "TensorBoard",
  jupyter: "Jupyter",
};

const serviceKey = (service: WebService) => `${service.kind}:${service.port}`;

/**
 * TensorBoard and Jupyter servers on the connected host, each opened in the
 * browser through a temporary SSH forward with one click.
 */
export function WebServices({ profile, connected }: { profile: SessionProfile; connected: boolean }) {
  const [open, setOpen] = useState(true);
  const [scan, setScan] = useState<WebServiceScan | null>(null);
  const [scanning, setScanning] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A reply for a previous profile must not land on this one.
  const generationRef = useRef(0);

  const refresh = useCallback(async () => {
    const generation = generationRef.current;
    setScanning(true);
    try {
      const next = await invoke<WebServiceScan>("scan_web_services", { sessionId: profile.id });
      if (generation === generationRef.current) {
        setScan(next);
        setError(null);
      }
    } catch (scanError) {
      if (generation === generationRef.current) setError(String(scanError));
    } finally {
      if (generation === generationRef.current) setScanning(false);
    }
  }, [profile.id]);

  useEffect(() => {
    generationRef.current += 1;
    setScan(null);
    setError(null);
    setScanning(false);
  }, [profile.id]);

  useEffect(() => {
    if (!open || !connected) {
      return;
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), SCAN_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [connected, open, refresh]);

  const openService = async (service: WebService) => {
    setOpening(serviceKey(service));
    setError(null);
    try {
      await invoke<string>("open_web_service", {
        sessionId: profile.id,
        host: service.host,
        port: service.port,
        path: service.path,
      });
    } catch (openError) {
      setError(String(openError));
    } finally {
      setOpening(null);
    }
  };

  if (!connected) {
    return null;
  }
  const services = scan?.services ?? [];

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
        {scan?.supported && <small>{services.length} running</small>}
      </button>
      {open && (
        <div className="port-forwards-body">
          {error && (
            <div className="port-forward-error" role="alert">
              {error}
            </div>
          )}
          {scan && !scan.supported && (
            <small className="port-forwards-empty">Detection supports Linux servers for now.</small>
          )}
          {scan?.supported && services.length === 0 && (
            <small className="port-forwards-empty">
              No TensorBoard or Jupyter running. Links printed in the terminal also open with
              ⌘/Ctrl-click.
            </small>
          )}
          {!scan && !error && <small className="port-forwards-empty">Looking for servers…</small>}
          {services.map((service) => {
            const key = serviceKey(service);
            const Icon = service.kind === "tensorboard" ? LineChart : BookOpen;
            return (
              <div className="port-forward-row web-service-row" key={key}>
                <Icon size={15} className={`web-service-icon ${service.kind}`} />
                <span className="port-forward-main">
                  <strong>
                    {KIND_LABELS[service.kind]}
                    <em>:{service.port}</em>
                  </strong>
                  {service.detail && <small title={service.detail}>{service.detail}</small>}
                </span>
                <span className="port-forward-actions">
                  <button
                    className="secondary-button compact"
                    type="button"
                    aria-label={`Open ${KIND_LABELS[service.kind]} on port ${service.port}`}
                    disabled={opening !== null}
                    onClick={() => void openService(service)}
                  >
                    {opening === key ? (
                      <LoaderCircle className="spin" size={13} />
                    ) : (
                      <ExternalLink size={13} />
                    )}
                    Open
                  </button>
                </span>
              </div>
            );
          })}
          {scan?.supported && (
            <div className="button-row">
              <button
                className="secondary-button compact"
                type="button"
                disabled={scanning}
                onClick={() => void refresh()}
              >
                <RefreshCw size={13} className={scanning ? "spin" : undefined} />
                Refresh
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
