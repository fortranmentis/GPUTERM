import type { GpuDetailMetric } from "../types/resourceDetails";

type GpuSelectorProps = {
  gpus: GpuDetailMetric[];
  selectedGpuUuid: string | null;
  onSelect: (gpuUuid: string) => void;
};

export function GpuSelector({
  gpus,
  selectedGpuUuid,
  onSelect,
}: GpuSelectorProps) {
  if (gpus.length <= 1) {
    return null;
  }

  return (
    <div className="gpu-detail-tabs" role="tablist" aria-label="GPU selector">
      {gpus.map((gpu) => {
        const selected = gpu.uuid === selectedGpuUuid;
        return (
          <button
            type="button"
            role="tab"
            aria-selected={selected}
            className={selected ? "selected" : ""}
            key={gpu.uuid}
            onClick={() => onSelect(gpu.uuid)}
          >
            GPU{gpu.index}
          </button>
        );
      })}
    </div>
  );
}
