import { useState } from "react";
import { BatchDisableSheet } from "../features/library/BatchDisableSheet";
import { createFixtureCatalogClient } from "../test-fixtures/catalog";

// Dense, isolated preview: no native calls or real activation changes.
const client = createFixtureCatalogClient();
const original = (await client.listSkills("all")).items[0];
const snapshot = await client.listTargetGroups(original.id);
const skills = Array.from({ length: 30 }, (_, index) => ({
  ...original,
  id: `preview-${index}`,
  name: `Skill ${index + 1}`,
}));
client.listTargetGroups = async (skillId) => ({
  ...snapshot,
  skillId,
  skillName: skills.find((skill) => skill.id === skillId)!.name,
});

export function BatchDisablePreview() {
  const [open, setOpen] = useState(true);
  return open ? (
    <BatchDisableSheet
      client={client}
      skills={skills}
      onClose={() => setOpen(false)}
    />
  ) : (
    <button onClick={() => setOpen(true)}>Open batch preview</button>
  );
}
