import "@testing-library/jest-dom/vitest";

// jsdom implements no layout, so it omits scrollIntoView entirely. This exists
// purely so tests can `vi.spyOn(Element.prototype, "scrollIntoView")` — spying
// fails with "scrollIntoView does not exist" otherwise. Callers do not depend on
// it: they use an optional call and are fine without it.
//
// Deliberately a plain no-op rather than a shared vi.fn(), because restoreMocks
// is not configured and a shared mock's calls would leak between test files.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = function scrollIntoView() {};
}
