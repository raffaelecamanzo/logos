// The action text the web UI no longer renders anywhere (CR-206, CR-207): the
// action line's label — also the header of the per-row action columns — and the
// sentence an empty row action read. One list, read by `expectWidgetCopy`, the
// widget source scan and the Playwright layout check, so the three refuse the
// same words. Test-only and free of any test-runner import, so the e2e specs can
// read it too.
export const REMOVED_ACTION_TEXT = ["What you can do", "Nothing to do — informational."] as const;
