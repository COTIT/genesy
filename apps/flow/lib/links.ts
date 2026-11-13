/**
 * Where a link the assistant shows may lead in Flow. Office builds its links as Office-relative
 * paths; Flow is another origin, so each one is checked against a fixed list and sent to the
 * Office origin (OFFICE_PUBLIC_URL, read on the server per request). Anything else, including
 * every absolute URL the model or a review field carries, is refused with "".
 */

/** Exactly one http(s) origin: no path, query, credentials or fragment. */
export function exactOrigin(value: string | undefined): string | undefined {
  try {
    const url = new URL(value ?? "")
    const exact =
      ["http:", "https:"].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      url.pathname === "/" &&
      !url.search &&
      !url.hash
    return exact ? url.origin : undefined
  } catch {
    return undefined
  }
}

/** The Office pages of the assistant's page tool (apps/office/lib/voice/pages.ts), minus the storefront, which Flow shows itself. */
export const OFFICE_PAGES: ReadonlySet<string> = new Set([
  "/",
  "/products",
  "/products/categories",
  "/stock",
  "/stock/negative",
  "/orders",
  "/invoices",
  "/lots",
  "/supply",
  "/supply/links",
  "/supply/transfers",
  "/till/sales",
  "/till/shifts",
  "/till/registers",
  "/till/users",
  "/till/vaults",
  "/settings/invites",
  "/settings/scheduled-reports",
  "/settings/app-keys",
  "/settings/webhooks",
  "/search",
])

/** One record page (apps/office/lib/voice/links.ts and lib/views.ts build these). */
const RECORD =
  /^\/(orders|invoices|products|lots)\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i
/** Office's secure form for one operation; the form asks Office's server for the rest. */
const SECURE = /^\/voice\/secure\?operation=[a-z0-9_]{1,64}$/

// Exact matches only: no prefix, decoding or normalising, so no path trick can widen the list.
export function flowHref(href: string, office: string | null): string {
  if (href === "/settings/storefront") return "/storefront"
  if (
    /^\/imports\/products\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      href
    )
  )
    return href
  if (!office) return ""
  if (OFFICE_PAGES.has(href) || SECURE.test(href)) return `${office}${href}`
  const record = RECORD.exec(href)
  return record ? `${office}/${record[1]}/${record[2]!.toLowerCase()}` : ""
}

/** A preview token as the API issues it: 64 lower-case hex characters, nothing else. */
const PREVIEW_TOKEN = /^[0-9a-f]{64}$/

/** The Shop's full-page preview of one link (SHOP_PUBLIC_URL, read on the server per request). */
export function shopPreviewHref(token: string, shop: string | null): string {
  return shop && PREVIEW_TOKEN.test(token) ? `${shop}/preview/${token}` : ""
}
