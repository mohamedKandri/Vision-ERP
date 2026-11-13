import { useState } from "react";
import { Button, Card, Field, Select, TextInput, Toggle } from "../components/ui";
import { errorText, LANG_NAMES, translate } from "../i18n";
import { rpc, type Lang } from "../lib/rpc";

export function Setup({ onDone }: { onDone: () => void }) {
  const [lang, setLang] = useState<Lang>("fr");
  const [shop, setShop] = useState("");
  const [owner, setOwner] = useState("");
  const [pin, setPin] = useState("");
  const [pin2, setPin2] = useState("");
  const [demo, setDemo] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const t = (k: Parameters<typeof translate>[1]) => translate(lang, k);

  const pinOk = /^\d{4,6}$/.test(pin);
  const valid = shop.trim() && owner.trim() && pinOk && pin === pin2;

  const submit = async () => {
    if (pin !== pin2) return setErr(t("pins_dont_match"));
    setBusy(true);
    setErr(null);
    try {
      await rpc("setup.init", { shop_name: shop, language: lang, owner_name: owner, owner_pin: pin, demo_users: demo });
      onDone();
    } catch (e) {
      setErr(errorText(lang, e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div dir={lang === "ar" ? "rtl" : "ltr"} className="flex min-h-full items-center justify-center p-4">
      <div className="w-full max-w-lg">
        <Card>
          <h1 className="text-2xl font-bold">🍔 {t("setup_title")}</h1>
          <p className="muted mb-5 mt-1">{t("setup_sub")}</p>
          <div className="flex flex-col gap-4">
            <Field label={t("language")}>
              <Select<Lang>
                value={lang}
                onChange={setLang}
                options={(Object.keys(LANG_NAMES) as Lang[]).map((l) => ({ value: l, label: LANG_NAMES[l] }))}
              />
            </Field>
            <Field label={t("shop_name")}>
              <TextInput value={shop} onChange={(e) => setShop(e.target.value)} autoFocus />
            </Field>
            <Field label={t("owner_name")}>
              <TextInput value={owner} onChange={(e) => setOwner(e.target.value)} />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label={t("owner_pin")}>
                <TextInput type="password" inputMode="numeric" maxLength={6} value={pin} onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))} />
              </Field>
              <Field label={t("confirm_pin")}>
                <TextInput type="password" inputMode="numeric" maxLength={6} value={pin2} onChange={(e) => setPin2(e.target.value.replace(/\D/g, ""))} />
              </Field>
            </div>
            <Toggle checked={demo} onChange={setDemo} label={t("demo_users")} />
            {err && <p className="text-red-500">{err}</p>}
            <Button variant="primary" disabled={!valid || busy} onClick={submit}>
              {t("start")}
            </Button>
          </div>
        </Card>
      </div>
    </div>
  );
}
