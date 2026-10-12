/*
 * The type of the build-time extension entry (S-642, FR-UI-51). `virtual:logos-extensions`
 * is not a file: `vite.config.ts` and `vitest.config.ts` alias it to the composed entry
 * (`composed.ts`), and this declaration is what lets `tsc -b` accept the import.
 */
declare module "virtual:logos-extensions" {
  const registrations: readonly import("./registry.ts").ExtensionUi[];
  export default registrations;
}
