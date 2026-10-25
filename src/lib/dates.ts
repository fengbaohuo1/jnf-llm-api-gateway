/** 最近 N 天日期（本地时区，YYYY-MM-DD，从旧到新，含今天） */
export function lastNDates(days: number, today = new Date()): string[] {
  const out: string[] = [];
  for (let i = days - 1; i >= 0; i -= 1) {
    const d = new Date(today.getFullYear(), today.getMonth(), today.getDate() - i);
    const month = String(d.getMonth() + 1).padStart(2, "0");
    const day = String(d.getDate()).padStart(2, "0");
    out.push(`${d.getFullYear()}-${month}-${day}`);
  }
  return out;
}
