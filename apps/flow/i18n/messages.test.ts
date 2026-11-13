import { describe, expect, it } from "vitest"
import { loadMessages, locales } from "@genesy/i18n/locales"
import { voiceMessages } from "@genesy/i18n/voice"
import { flowMessages } from "./request"

describe("Flow messages", () => {
  it.each(locales)(
    "%s hosts the shared voice texts and keeps Flow's own",
    async (locale) => {
      const messages = await loadMessages(locale, flowMessages)
      expect(messages.voice).toEqual(voiceMessages[locale].voice)
      expect(messages.flow).toBeDefined()
    }
  )
})

const keys = (value: unknown, prefix = ""): string[] =>
  value && typeof value === "object"
    ? Object.entries(value).flatMap(([key, item]) =>
        keys(item, `${prefix}${key}.`)
      )
    : [prefix]

describe("Flow namespace files", () => {
  // F2: scenes.json
  it.each(
    locales.flatMap(
      (l) =>
        [
          [l, "shell"],
          [l, "scenes"],
          [l, "history"],
          [l, "productBulk"],
        ] as const
    )
  )("%s merges %s.json with the same keys as English", async (locale, name) => {
    const english = await loadMessages("en", flowMessages)
    const messages = await loadMessages(locale, flowMessages)
    expect(english[name]).toBeDefined()
    expect(keys(messages[name]).sort()).toEqual(keys(english[name]).sort())
  })
})
