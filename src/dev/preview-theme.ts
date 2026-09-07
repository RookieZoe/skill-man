/** Reuse the production media rule for browser-only fixture screenshots when
 * the browser host cannot emulate color-scheme. Never loaded in production. */
export function previewDarkTheme() {
  const declarations = Array.from(document.styleSheets).flatMap((sheet) =>
    Array.from(sheet.cssRules)
      .filter(
        (rule) =>
          rule instanceof CSSMediaRule &&
          rule.conditionText === "(prefers-color-scheme: dark)",
      )
      .flatMap((rule) =>
        Array.from((rule as CSSMediaRule).cssRules).map(
          (child) => child.cssText,
        ),
      ),
  );
  const style = document.createElement("style");
  style.textContent = declarations.join("\n");
  document.head.append(style);
}
