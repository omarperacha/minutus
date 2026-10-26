import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { AssetId } from "../generated/ipc";

type ManagedImagePreviewProps = {
  assetId: AssetId;
  alt: string;
};

export function ManagedImagePreview({ assetId, alt }: ManagedImagePreviewProps) {
  const [source, setSource] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    let objectUrl: string | null = null;
    setSource(null);
    setFailed(false);

    invoke<ArrayBuffer>("managed_image_preview", { assetId })
      .then((bytes) => {
        if (!active) return;
        objectUrl = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
        setSource(objectUrl);
      })
      .catch(() => {
        if (active) setFailed(true);
      });

    return () => {
      active = false;
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
    };
  }, [assetId]);

  if (failed) return <span className="attachment-preview-error">Preview unavailable</span>;
  if (source === null) return <span className="attachment-preview-loading" aria-label="Loading image preview" />;
  return <img src={source} alt={alt} />;
}
