import { useEffect, useState } from "react";

interface Props {
  value: number;
  onChange: (value: number) => void;
  disabled?: boolean;
  title?: string;
  label?: string;
}

const MAXIMUM = 2;
const STEP = 0.05;

/**
 * The text box for a LoRA file's strength, from 0 to 2. The user can start
 * the number with the decimal point (".5"); a number box does not accept
 * that. The box keeps the text as the user writes it, and gives each good
 * number to the owner at once. The up and down arrow keys change the
 * strength by 0.05.
 */
export function StrengthInput({ value, onChange, disabled, title, label }: Props) {
  const [text, setText] = useState(String(value));

  /* The owner can change the value, for example when the list's sequence changes. Text that is the same number stays as the user wrote it. */
  useEffect(() => {
    setText((current) => (Number(current) === value && current.trim() !== "" ? current : String(value)));
  }, [value]);

  const limit = (number: number) => Math.min(MAXIMUM, Math.max(0, Math.round(number * 100) / 100));

  return (
    <input
      type="text"
      inputMode="decimal"
      className="strength-input"
      spellCheck={false}
      value={text}
      disabled={disabled}
      title={title}
      aria-label={label}
      onChange={(event) => {
        const next = event.target.value.replace(",", ".").trim();
        if (!/^\d*\.?\d*$/.test(next)) return;
        setText(next);
        const number = Number(next);
        if (next !== "" && next !== "." && Number.isFinite(number)) onChange(limit(number));
      }}
      onBlur={() => {
        const number = Number(text);
        const kept = text === "" || text === "." || !Number.isFinite(number) ? value : limit(number);
        setText(String(kept));
        if (kept !== value) onChange(kept);
      }}
      onKeyDown={(event) => {
        if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
        event.preventDefault();
        const next = limit(value + (event.key === "ArrowUp" ? STEP : -STEP));
        setText(String(next));
        onChange(next);
      }}
    />
  );
}
