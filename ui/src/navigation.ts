/** Switches local settings views without touching the active audio session. */
export function initializeNavigation(root: Document): void {
  const buttons = Array.from(root.querySelectorAll<HTMLButtonElement>("button[data-view]"));
  for (const button of buttons) {
    button.addEventListener("click", () => {
      const view = button.dataset.view;
      for (const candidate of buttons) {
        const selected = candidate.dataset.view === view;
        if (selected) candidate.setAttribute("aria-current", "page");
        else candidate.removeAttribute("aria-current");
        const panel = root.getElementById(`view-${candidate.dataset.view}`);
        if (panel) panel.hidden = !selected;
      }
    });
  }
}
