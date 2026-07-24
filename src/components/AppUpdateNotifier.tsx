import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  installAvailableAppUpdate,
  runAutomaticAppUpdateCheck,
  tauriAppUpdaterRuntime,
  type AppUpdaterRuntime,
} from "../lib/appUpdater";

const APP_UPDATE_TOAST_ID = "app-update-available";
export const APP_UPDATE_STARTUP_DELAY_MS = 3000;

interface AppUpdateNotifierProps {
  runtime?: AppUpdaterRuntime;
  delayMs?: number;
}

export function AppUpdateNotifier({
  runtime = tauriAppUpdaterRuntime,
  delayMs = APP_UPDATE_STARTUP_DELAY_MS,
}: AppUpdateNotifierProps) {
  const { t } = useTranslation();

  useEffect(() => {
    let active = true;
    const showUpdate = (version: string) => {
      if (!active) return;
      toast.info(t("settings.appUpdate.ready", { version }), {
        id: APP_UPDATE_TOAST_ID,
        duration: Infinity,
        action: {
          label: t("settings.appUpdate.installAndRestart"),
          onClick: async () => {
            toast.loading(t("settings.appUpdate.installing"), { id: APP_UPDATE_TOAST_ID });
            try {
              const installResult = await installAvailableAppUpdate(runtime, version);
              if (installResult.status === "no-update") {
                toast.success(t("settings.appUpdate.noLongerAvailable"), {
                  id: APP_UPDATE_TOAST_ID,
                });
              } else if (installResult.status === "version-changed") {
                showUpdate(installResult.version);
              }
            } catch (error) {
              runtime.logError("App update installation failed", error);
              toast.error(t("settings.appUpdate.installFailed"), {
                id: APP_UPDATE_TOAST_ID,
              });
            }
          },
        },
      });
    };

    const timer = window.setTimeout(() => {
      void runAutomaticAppUpdateCheck(runtime).then((result) => {
        if (!active || result.status !== "update-available") return;
        showUpdate(result.version);
      });
    }, delayMs);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [delayMs, runtime, t]);

  return null;
}
