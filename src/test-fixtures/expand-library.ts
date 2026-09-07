import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

/** Tests that interact with member rows explicitly open the collapsed groups. */
export async function expandLibrary() {
  await screen.findByRole("heading", { name: "skill-authoring" });
  for (const button of document.querySelectorAll<HTMLButtonElement>(
    '.library-source-heading[aria-expanded="false"]',
  ))
    await userEvent.click(button);
}
