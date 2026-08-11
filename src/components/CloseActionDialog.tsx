import { useState } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";

interface Props {
  open: boolean;
  onCancel: () => void;
  onClose: (remember: boolean) => void;
  onHide: (remember: boolean) => void;
}

export function CloseActionDialog({ open, onCancel, onClose, onHide }: Props) {
  const { t } = useTranslation();
  const [remember, setRemember] = useState(false);

  const handleCancel = () => {
    setRemember(false);
    onCancel();
  };

  const handleClose = () => {
    onClose(remember);
    setRemember(false);
  };

  const handleHide = () => {
    onHide(remember);
    setRemember(false);
  };

  if (!open) return null;

  return (
    <div className="app-dialog-layer">
      <div className="app-dialog-backdrop" onClick={handleCancel} />
      <div className="app-dialog w-full max-w-sm p-5">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-[13px] font-semibold text-primary">
            {t("closeAction.title")}
          </h2>
          <button
            onClick={handleCancel}
            className="text-muted hover:text-secondary p-1 rounded transition-colors outline-none"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <p className="text-[13px] text-tertiary mb-4">{t("closeAction.message")}</p>

        <label className="flex items-center gap-2 mb-5 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
            className="w-3.5 h-3.5 accent-[var(--color-accent)]"
          />
          <span className="text-[13px] text-muted">{t("closeAction.remember")}</span>
        </label>

        <div className="flex justify-end gap-2">
          <button
            onClick={handleClose}
            className="app-button-secondary h-8 px-3"
          >
            {t("closeAction.close")}
          </button>
          <button
            onClick={handleHide}
            className="app-button-primary h-8 px-3"
          >
            {t("closeAction.hide")}
          </button>
        </div>
      </div>
    </div>
  );
}
