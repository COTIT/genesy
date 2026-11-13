import { describe, expect, it } from "vitest"

import { exactOrigin, flowHref, OFFICE_PAGES, shopPreviewHref } from "./links"

const OFFICE = "https://office.genesy.test"
const ID = "0193f6b0-1c2d-7e3f-8a9b-0c1d2e3f4a5b"

describe("exactOrigin", () => {
  it.each([
    ["https://office.genesy.app", "https://office.genesy.app"],
    ["https://office.genesy.app/", "https://office.genesy.app"],
    ["http://localhost:3001", "http://localhost:3001"],
  ])("keeps %s", (value, origin) => {
    expect(exactOrigin(value)).toBe(origin)
  })

  it.each([
    undefined,
    "",
    "office.genesy.app",
    "ftp://office.genesy.app",
    "javascript:alert(1)",
    "https://office.genesy.app/path",
    "https://office.genesy.app/?x=1",
    "https://office.genesy.app/#x",
    "https://user:pass@office.genesy.app",
  ])("refuses %s", (value) => {
    expect(exactOrigin(value)).toBeUndefined()
  })
})

describe("flowHref", () => {
  it("keeps exact migration reviews in Flow without trusting arbitrary routes", () => {
    expect(flowHref(`/imports/products/${ID}`, null)).toBe(
      `/imports/products/${ID}`
    )
    for (const suffix of [
      "/../settings",
      "?approve=true",
      "/approve",
      "#confirmation",
    ])
      expect(flowHref(`/imports/products/${ID}${suffix}`, OFFICE)).toBe("")
  })
  // Same list as apps/office/lib/voice/pages.ts; a page added there needs adding here.
  it("knows every Office page the assistant's page tool can open", () => {
    expect([...OFFICE_PAGES].sort()).toEqual(
      [
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
      ].sort()
    )
  })

  it.each([...OFFICE_PAGES])(
    "opens Office page %s on the Office origin",
    (path) => {
      expect(flowHref(path, OFFICE)).toBe(`${OFFICE}${path}`)
    }
  )

  it.each(["orders", "invoices", "products", "lots"])(
    "opens one %s record on the Office origin, id in lower case",
    (resource) => {
      expect(flowHref(`/${resource}/${ID.toUpperCase()}`, OFFICE)).toBe(
        `${OFFICE}/${resource}/${ID}`
      )
    }
  )

  it("opens the storefront design in Flow itself", () => {
    expect(flowHref("/settings/storefront", OFFICE)).toBe("/storefront")
    expect(flowHref("/settings/storefront", null)).toBe("/storefront")
  })

  it("turns a secure-form handoff into Office's secure page, nothing added", () => {
    expect(flowHref("/voice/secure?operation=app_keys_create", OFFICE)).toBe(
      `${OFFICE}/voice/secure?operation=app_keys_create`
    )
  })

  it.each([
    "https://evil.example/orders",
    "http://evil.example",
    `${OFFICE}/orders`,
    "//evil.example/orders",
    "/\\evil.example",
    "\\\\evil.example",
    "javascript:alert(1)",
    "JavaScript:alert(1)",
    "data:text/html,hi",
    "mailto:a@b.c",
    "/orders/../settings/app-keys",
    "/orders/%2e%2e/settings",
    "/orders%2F" + ID,
    "/%2F%2Fevil.example",
    "/orders/not-a-uuid",
    "/customers/" + ID,
    `/orders/${ID}/edit`,
    `/orders/${ID}?tab=lines`,
    `/orders/${ID}#lines`,
    "/orders?status=open",
    "/settings",
    "/settings/password",
    "/settings/storefront?draft=1",
    "/storefront",
    "/api/voice/session",
    "/v1/me",
    "/voice/secure",
    "/voice/secure?operation=",
    "/voice/secure?operation=../x",
    "/voice/secure?operation=APP_KEYS",
    "/voice/secure?operation=app_keys_create&token=secret",
    "/voice/secure?token=secret&operation=app_keys_create",
    "/voice/secure?operation=a&operation=b",
    " /orders",
    "/orders\n",
    "/ord\u0000ers",
    "",
    "orders",
  ])("refuses %j", (href) => {
    expect(flowHref(href, OFFICE)).toBe("")
  })

  it("refuses every Office address when Flow has no Office origin", () => {
    expect(flowHref("/orders", null)).toBe("")
    expect(flowHref(`/orders/${ID}`, null)).toBe("")
    expect(flowHref("/voice/secure?operation=app_keys_create", null)).toBe("")
  })
})

describe("shopPreviewHref", () => {
  const SHOP = "https://shop.genesy.test"
  const TOKEN = "0123456789abcdef".repeat(4)

  it("opens /preview/<token> on the Shop origin", () => {
    expect(shopPreviewHref(TOKEN, SHOP)).toBe(`${SHOP}/preview/${TOKEN}`)
  })

  it("refuses without a Shop origin", () => {
    expect(shopPreviewHref(TOKEN, null)).toBe("")
  })

  it.each([
    "",
    TOKEN.slice(1),
    TOKEN + "0",
    TOKEN.toUpperCase(),
    "g".repeat(64),
    `../${TOKEN.slice(3)}`,
    `${TOKEN.slice(1)}/`,
    `${TOKEN.slice(1)}?`,
    `${TOKEN.slice(1)}#`,
  ])("refuses token %j", (token) => {
    expect(shopPreviewHref(token, SHOP)).toBe("")
  })
})
