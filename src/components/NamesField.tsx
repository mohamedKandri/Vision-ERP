import { useT } from "../i18n";
import type { Names } from "../lib/menu";
import { Field, TextInput } from "./ui";

/** Three inputs for a translated name. French is required; the others fall back to French. */
export function NamesField({ value, onChange, autoFocus }: { value: Partial<Names>; onChange: (n: Partial<Names>) => void; autoFocus?: boolean }) {
  const t = useT();
  return (
    <div className="grid gap-3 sm:grid-cols-3">
      <Field label={t("name_fr")}>
        <TextInput dir="ltr" value={value.fr ?? ""} onChange={(e) => onChange({ ...value, fr: e.target.value })} autoFocus={autoFocus} />
      </Field>
      <Field label={t("name_ar")}>
        <TextInput dir="rtl" value={value.ar ?? ""} onChange={(e) => onChange({ ...value, ar: e.target.value })} />
      </Field>
      <Field label={t("name_en")}>
        <TextInput dir="ltr" value={value.en ?? ""} onChange={(e) => onChange({ ...value, en: e.target.value })} />
      </Field>
    </div>
  );
}
