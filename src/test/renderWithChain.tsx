import type { ReactElement } from "react";
import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { ChainProvider } from "../context/ChainContext";

/**
 * Render a chain work area the way the app mounts it: inside the shared scan
 * provider, and inside a router because every area can navigate.
 *
 * The provider runs the same `invoke` calls the tests already stub, so a view
 * test still controls its own data through the `invoke` mock.
 */
export function renderWithChain(ui: ReactElement, initialEntry = "/") {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <ChainProvider>{ui}</ChainProvider>
    </MemoryRouter>,
  );
}
