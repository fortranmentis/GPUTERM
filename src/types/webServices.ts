export type WebServiceKind = "tensorboard" | "jupyter";

/** A TensorBoard or Jupyter server on a connected host (`scan_web_services`). */
export type WebService = {
  kind: WebServiceKind;
  pid: number | null;
  /** Where the server connects for the forward. */
  host: string;
  port: number;
  /** Path and query to open, including Jupyter's login token. */
  path: string;
  /** TensorBoard's logdir or Jupyter's root directory. */
  detail: string | null;
};

export type WebServiceScan = {
  /** False on hosts detection does not cover yet (macOS, Windows). */
  supported: boolean;
  services: WebService[];
};
