import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "../i18n/en.json";

// Initialise i18n synchronously with the English bundle so `t()` returns real
// strings in tests, bypassing the app's async, Tauri-backed language loader.
if (!i18n.isInitialized) {
  void i18n.use(initReactI18next).init({
    resources: { en: { translation: en } },
    lng: "en",
    fallbackLng: "en",
    interpolation: { escapeValue: false },
  });
}

afterEach(() => cleanup());
