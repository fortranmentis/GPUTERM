import { describe, expect, it } from "vitest";
import { isServingEngine, LLM_RUNTIMES, runtimeLabel } from "./llmRuntimes";

describe("llmRuntimes", () => {
  it("labels every runtime the backend knows", () => {
    expect(LLM_RUNTIMES.map((runtime) => runtime.key)).toEqual([
      "ollama",
      "vllm",
      "sglang",
      "tgi",
      "llamacpp",
      "openai",
      "triton",
    ]);
    expect(runtimeLabel("llamacpp")).toBe("llama.cpp");
  });

  it("matches the backend's serving-engine grading", () => {
    expect(isServingEngine("ollama")).toBe(false);
    expect(isServingEngine("openai")).toBe(false);
    expect(isServingEngine("sglang")).toBe(true);
    expect(isServingEngine("triton")).toBe(true);
  });
});
