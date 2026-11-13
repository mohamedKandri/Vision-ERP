import { describe, expect, it } from "vitest";
import { DICTS, translate } from "./index";

describe("i18n", () => {
  const base = Object.keys(DICTS.fr).sort();
  it.each(["en", "ar"] as const)("%s has exactly the French keys, none empty", (lang) => {
    expect(Object.keys(DICTS[lang]).sort()).toEqual(base);
    for (const [k, v] of Object.entries(DICTS[lang])) expect(v.trim(), k).not.toBe("");
  });
  it("placeholders survive in every language", () => {
    for (const lang of ["fr", "en", "ar"] as const) {
      expect(translate(lang, "locked", { s: 42 })).toContain("42");
      expect(translate(lang, "coming_phase", { n: 3 })).toContain("3");
    }
  });
});
