import { describe, expect, it } from "vitest"

import { discoverTools, operationManifest } from "./catalog"
import grandfathered from "./keywords.grandfathered.json"
import keywords from "./keywords.json"

// RUAHA co-op admin: workspace admin plus live farm admin role.
const admin = ["workspace_admin", "farm:admin", "farm:ops"]
const top = (query: string, n = 3) =>
  discoverTools(query, admin)
    .slice(0, n)
    .map((tool) => tool.id)

describe("voice trigger words", () => {
  it("names only real catalog operations, each with en/nl/sw word lists", () => {
    const ids = new Set(operationManifest.map((op) => op.id))
    for (const [id, words] of Object.entries(keywords)) {
      expect(ids.has(id), id).toBe(true)
      expect(Object.keys(words).sort(), id).toEqual(["en", "nl", "sw"])
      for (const list of Object.values(words))
        for (const word of list)
          expect(word, id).toMatch(/^[\p{Ll}\p{N}' -]+$/u)
    }
  })

  // G3: a new operation voice can run ships with words; the ops from before this rule are listed and only shrink.
  it("gives every executable operation trigger words, unless it predates the rule", () => {
    const words = keywords as Record<string, Record<string, string[]>>
    const old = new Set(grandfathered)
    const missing = operationManifest
      .filter((op) => op.disposition === "execute")
      .filter((op) => !words[op.id] && !old.has(op.id))
      .map((op) => op.id)
    expect(missing).toEqual([])
  })

  it("lists only grandfathered operations that still lack words", () => {
    const executable = new Set(
      operationManifest
        .filter((op) => op.disposition === "execute")
        .map((op) => op.id)
    )
    const healed = grandfathered.filter(
      (id) => id in keywords || !executable.has(id)
    )
    expect(healed).toEqual([])
    expect([...grandfathered].sort()).toEqual(grandfathered)
  })

  it.each([
    // en: queries the probe agent actually sent when it said "can't"
    [
      "record or create delivery received 40 kg coffee cherry from farmer in Kitowo; coffee deliveries intake farmer produce",
      "farm_supply_create",
    ],
    ["farmer delivered 200 kg coffee today", "farm_supply_create"],
    [
      "farmers outstanding balance unpaid amount owed to farmers",
      "farm_passbook_person",
    ],
    ["farmer passbook statement for October", "farm_passbook_person"],
    [
      "inventory stock transfer between branches; transfer quantity of product to another branch",
      "supply_chain_dispatch",
    ],
    ["send coffee to the other branch", "supply_chain_dispatch"],
    ["set up a supply link with a new farm co-op", "supply_links_grant"],
    // v1 re-probe: the agent's own queries
    ["create supply link new farm co-op", "supply_links_grant"],
    ["farmer passbook transactions October Mwangeza", "farm_passbook_person"],
    [
      "farmer Kitowo supply link consent data sharing buyers",
      "farm_passbook_consent",
    ],
    ["pay farmers what we owe", "farm_payouts_create"],
    [
      "correct the delivery weight of a farmer harvest",
      "farm_records_harvest_reports_revise",
    ],
    ["how much do we owe farmers in total", "farm_passbook_balances"],
    ["did the farmer agree to share data", "farm_passbook_consent"],
    ["farmers who haven't delivered in a month", "farm_supply_quiet_farmers"],
    // sw
    ["mkulima ameleta kilo 200 za kahawa leo", "farm_supply_create"],
    ["jumla ya deni la wakulima", "farm_passbook_balances"],
    ["ridhaa ya mkulima kushiriki taarifa", "farm_passbook_consent"],
    ["wakulima hawajaleta tangu mwezi", "farm_supply_quiet_farmers"],
    ["deni la mkulima daftari", "farm_passbook_person"],
    ["peleka kahawa tawi lingine", "supply_chain_dispatch"],
    ["unganisha na ushirika mwingine", "supply_links_grant"],
    ["lipa wakulima malipo", "farm_payouts_create"],
    // nl
    ["boer heeft 200 kg koffie geleverd", "farm_supply_create"],
    ["hoeveel zijn we de boer schuldig", "farm_passbook_person"],
    ["koffie naar het andere filiaal sturen", "supply_chain_dispatch"],
    // scheduled reports (v1 gap: recurring email, stock alert)
    [
      "every Monday morning send me a sales summary by email",
      "scheduled_reports_create",
    ],
    ["tell me when eggs drop below 10 trays", "scheduled_reports_create"],
    ["stop my weekly sales email", "scheduled_reports_update"],
    ["stuur me elke maandag een verkoopoverzicht", "scheduled_reports_create"],
    [
      "kila Jumatatu asubuhi nitumie muhtasari wa mauzo kwa barua pepe",
      "scheduled_reports_create",
    ],
    // currency: workspace rates, convert read
    ["convert this invoice to euros", "currency_rates_convert"],
    ["what is that in TZS", "currency_rates_convert"],
    ["set the exchange rate from euro to shilling", "currency_rates_create"],
    ["show the exchange rate history", "currency_rates_list"],
    ["badilisha kiasi hiki kwa euro", "currency_rates_convert"],
    ["weka kiwango cha kubadilisha fedha", "currency_rates_create"],
    ["reken dit om naar euro", "currency_rates_convert"],
    ["wisselkoers instellen voor de shilling", "currency_rates_create"],
    // revenue excl VAT, top buyer of one product (v1 re-probe gaps)
    ["what's our revenue this month, excluding VAT?", "orders_sales_summary"],
    ["net sales ex VAT for October", "orders_sales_summary"],
    ["omzet deze maand ex btw", "orders_sales_summary"],
    ["mapato ya mwezi huu bila VAT", "orders_sales_summary"],
    // Genesy Flow (v9): noticed priorities, marks, daily sales, own scenes
    ["what needs my attention today", "attention_summary"],
    [
      "what should I look at first, most important priorities",
      "attention_priorities",
    ],
    ["which products are below their minimum stock", "attention_priorities"],
    ["wat is het belangrijkste, prioriteiten", "attention_priorities"],
    ["kipaumbele gani ni muhimu kwanza", "attention_priorities"],
    ["snooze this alert until tomorrow", "attention_marks_set"],
    ["mark it as seen, acknowledge it", "attention_marks_set"],
    ["stel deze melding uit tot morgen", "attention_marks_set"],
    ["ahirisha tahadhari hii hadi kesho", "attention_marks_set"],
    ["unsnooze that alert, show it again", "attention_marks_delete"],
    ["zet die melding terug", "attention_marks_delete"],
    ["rudisha tahadhari ile tena", "attention_marks_delete"],
    ["sales per day this week as a chart", "orders_sales_daily"],
    ["daily sales trend for the last two weeks", "orders_sales_daily"],
    ["verkoop per dag deze week", "orders_sales_daily"],
    ["mauzo kwa siku wiki hii", "orders_sales_daily"],
    ["what is on my flow scenes", "flow_scenes_list"],
    ["welke kaarten staan in mijn flow overzicht", "flow_scenes_list"],
    ["kadi gani ziko kwenye dashibodi yangu", "flow_scenes_list"],
    [
      "always show pending orders first on my pulse scene",
      "flow_scenes_update",
    ],
    [
      "toon altijd openstaande bestellingen eerst in mijn flow",
      "flow_scenes_update",
    ],
    [
      "onyesha kila mara oda zinazosubiri kwanza kwenye dashibodi",
      "flow_scenes_update",
    ],
    ["put my stock scene back to the default", "flow_scenes_delete"],
    ["zet mijn voorraad scène terug naar standaard", "flow_scenes_delete"],
    ["rejesha mpangilio wa stoo kwa chaguo-msingi", "flow_scenes_delete"],
    ["reset my whole flow layout", "flow_scenes_reset"],
    ["mijn flow indeling resetten", "flow_scenes_reset"],
    ["weka upya mpangilio wangu wote", "flow_scenes_reset"],
    ["who buys the most cardamom", "orders_top_customers"],
    ["who is our top customer for cardamom", "orders_top_customers"],
    ["wie koopt de meeste kardemom", "orders_top_customers"],
    ["nani ananunua iliki zaidi", "orders_top_customers"],
    // storefront design (Phase 3)
    ["change the headline on my shop home page", "storefront_design_edit"],
    ["make the storefront theme neutral", "storefront_design_edit"],
    ["publish the new storefront design", "storefront_design_publish"],
    [
      "undo the last change to the shop page design",
      "storefront_design_restore",
    ],
    ["what can I change on my storefront", "storefront_design_capabilities"],
    ["verander de kop van de webwinkel", "storefront_design_edit"],
    ["publiceer het nieuwe winkelontwerp", "storefront_design_publish"],
    ["badilisha kichwa cha ukurasa wa duka", "storefront_design_edit"],
    ["chapisha muundo mpya wa duka", "storefront_design_publish"],
    // Office parity P1: order steps and batch status in the words people use on screen
    ["confirm order 1042", "orders_confirm"],
    ["bevestig bestelling 1042", "orders_confirm"],
    ["thibitisha oda 1042", "orders_confirm"],
    ["record picking for order 1042", "orders_pick"],
    ["picking invoeren voor bestelling 1042", "orders_pick"],
    ["rekodi bidhaa zilizokusanywa kwa oda 1042", "orders_pick"],
    ["recall batch L-12", "lots_update"],
    ["release batch L-12 from quarantine", "lots_update"],
    ["put batch L-12 on hold in quarantine", "lots_update"],
    ["partij L-12 terugroepen", "lots_update"],
    ["partij L-12 in quarantaine zetten", "lots_update"],
    ["rudisha bechi L-12 sokoni", "lots_update"],
    ["weka bechi L-12 karantini", "lots_update"],
  ])("finds the right operation for %j", (query, id) => {
    expect(top(query)).toContain(id)
  })

  // Words match only at a word start, so a Dutch compound needs its own trigger word.
  it.each([
    ["kassaverkopen van vandaag", "till_sales_list"],
    ["voorraadniveau van honing", "stock_list"],
    ["verkooporders van deze week", "orders_list"],
    ["openstaande facturen", "invoices_list"],
    ["magazijnlocatie van de honing", "stock_with_bins"],
    ["betaaltermijn van de factuur aanpassen", "invoices_update"],
    ["kassagebruiker toevoegen", "till_users_create"],
    ["productcategorieën laten zien", "product_categories_list"],
  ])("finds the operation for the Dutch compound in %j", (query, id) => {
    expect(top(query)).toContain(id)
  })

  it.each([
    ["list product custom field definitions", "product_fields_list"],
    ["toon productvelden en velddefinities", "product_fields_list"],
    ["orodha ya sehemu maalum za bidhaa", "product_fields_list"],
    ["show custom field definition details", "product_fields_get"],
    ["toon details van dit productveld", "product_fields_get"],
    ["onyesha maelezo ya sehemu maalum ya bidhaa", "product_fields_get"],
  ])("finds the product field operation for %j", (query, id) => {
    expect(top(query)).toContain(id)
  })

  // A wrong delivery is a correction request, not a new purchase.
  it.each([
    "correct Gift's delivery to 40 kg",
    "the scale was wrong, fix Juma's delivery to 35 kilos",
    "corrigeer de levering van Gift naar 40 kilo",
    "rekebisha uwasilishaji wa Gift kuwa kilo 40",
  ])("puts the delivery correction first for %j", (query) => {
    expect(top(query, 1)).toEqual(["farm_supply_correction_request"])
  })

  // Same account, two warehouses: the in-account move wins over the cross-account supply transfer.
  it.each([
    "move 20 bags of maize from the main store to the shop",
    "transfer stock from the warehouse to the shop",
    "verplaats 20 zakken mais van het hoofdmagazijn naar de winkel",
    "hamisha mifuko 20 ya mahindi kutoka stoo kuu kwenda dukani",
  ])("puts the in-account stock transfer first for %j", (query) => {
    expect(top(query, 1)).toEqual(["stock_transfer"])
  })
})
