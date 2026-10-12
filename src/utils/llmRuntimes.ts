import type { LlmRuntimeType } from "../types/llm";

export type LlmRuntimeInfo = {
  key: LlmRuntimeType;
  label: string;
  exampleUrl: string;
  /** Reports request queues, KV cache and token rates (vLLM-style metrics). */
  servingEngine: boolean;
};

/** Mirrors `RuntimeType` in src-tauri/src/llm/adapter.rs. */
export const LLM_RUNTIMES: LlmRuntimeInfo[] = [
  { key: "ollama", label: "Ollama", exampleUrl: "http://192.168.0.20:11434", servingEngine: false },
  { key: "vllm", label: "vLLM", exampleUrl: "http://192.168.0.21:8000", servingEngine: true },
  { key: "sglang", label: "SGLang", exampleUrl: "http://192.168.0.21:30000", servingEngine: true },
  { key: "tgi", label: "TGI", exampleUrl: "http://192.168.0.21:8080", servingEngine: true },
  { key: "llamacpp", label: "llama.cpp", exampleUrl: "http://192.168.0.21:8080", servingEngine: true },
  {
    key: "openai",
    label: "OpenAI-compatible",
    exampleUrl: "http://192.168.0.20:1234",
    servingEngine: false,
  },
  { key: "triton", label: "Triton", exampleUrl: "http://192.168.0.21:8000", servingEngine: true },
];

function runtimeInfo(runtime: LlmRuntimeType): LlmRuntimeInfo | undefined {
  return LLM_RUNTIMES.find((item) => item.key === runtime);
}

export function runtimeLabel(runtime: LlmRuntimeType): string {
  return runtimeInfo(runtime)?.label ?? runtime;
}

export function isServingEngine(runtime: LlmRuntimeType): boolean {
  return runtimeInfo(runtime)?.servingEngine ?? true;
}

export function runtimeExampleUrl(runtime: LlmRuntimeType): string {
  return runtimeInfo(runtime)?.exampleUrl ?? "http://192.168.0.21:8000";
}
