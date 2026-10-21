// Post-login redirect targets come from the `?redirect=` query parameter, which an
// attacker controls. Only a same-origin, root-relative path may be followed.

// Must start with a single "/": "//host" is protocol-relative and "/\host" is read
// as "//host" by browsers (backslash is treated as a slash in special URLs).
const ROOT_RELATIVE_PATH = /^\/(?![/\\])/;

// Browsers strip tab, CR and LF from URLs before parsing, so "/\t/host" becomes
// "//host"; reject control characters and any backslash outright.
const UNSAFE_CHARACTERS = /[\u0000-\u001f\u007f\\]/;

export function safeRedirectPath(
  candidate: string | null | undefined,
  fallback = "/",
): string {
  if (typeof candidate !== "string") return fallback;
  if (!ROOT_RELATIVE_PATH.test(candidate) || UNSAFE_CHARACTERS.test(candidate)) {
    return fallback;
  }
  try {
    // Last line of defence: resolve it the way the router will and confirm it
    // stays on this origin and is not itself protocol-relative after normalisation.
    const base = new URL("http://redirect.invalid");
    const resolved = new URL(candidate, base);
    if (resolved.origin !== base.origin || resolved.pathname.startsWith("//")) {
      return fallback;
    }
  } catch {
    return fallback;
  }
  return candidate;
}
