export function replaceSpelling(text, heard, wanted) {
  if (!heard.trim() || !wanted.trim()) return text;
  const pattern = new RegExp(
    heard.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
    "gu",
  );
  const word = /[\p{L}\p{N}_]/u;
  return text.replace(pattern, (match, at) => {
    const before = [...text.slice(Math.max(0, at - 2), at)].at(-1) || "";
    const after =
      text
        .slice(at + match.length)
        [Symbol.iterator]()
        .next().value || "";
    return word.test(before) || word.test(after) ? match : wanted;
  });
}

export function suggestSpelling(before, after) {
  if (before === after || before.length > 16000 || after.length > 16000)
    return null;
  const words = (text) => [
    ...text.matchAll(/[\p{L}\p{N}]+(?:['’][\p{L}\p{N}]+)*/gu),
  ];
  const a = words(before),
    b = words(after);
  let prefix = 0;
  while (
    prefix < a.length &&
    prefix < b.length &&
    a[prefix][0] === b[prefix][0]
  )
    prefix++;
  if (!a[prefix] || !b[prefix]) return null;
  for (let i = 1; i <= 3 && a[prefix + i - 1]; i++) {
    for (let j = 1; j <= 3 && b[prefix + j - 1]; j++) {
      const heard = before.slice(
        a[prefix].index,
        a[prefix + i - 1].index + a[prefix + i - 1][0].length,
      );
      const wanted = after.slice(
        b[prefix].index,
        b[prefix + j - 1].index + b[prefix + j - 1][0].length,
      );
      if (replaceSpelling(before, heard, wanted) === after)
        return { heard, wanted };
    }
  }
  return null;
}
