export type WebServiceKind =
  | "tensorboard"
  | "jupyter"
  | "mlflow"
  | "ray"
  | "streamlit"
  | "gradio"
  | "code-server"
  /** A listening port of the user's that is not one of the above. */
  | "other";

/** A web UI on a connected host (`scan_web_services`). */
export type WebService = {
  kind: WebServiceKind;
  pid: number | null;
  /** Where the server connects for the forward: "localhost", or a container's address. */
  host: string;
  port: number;
  /** Path and query to open, including Jupyter's login token. */
  path: string;
  /** TensorBoard's logdir, Jupyter's root directory, or the program. */
  detail: string | null;
  /** The Docker container serving it, if any. */
  container: string | null;
};

export type WebServiceScan = {
  /** False for the local terminal. */
  supported: boolean;
  services: WebService[];
};
