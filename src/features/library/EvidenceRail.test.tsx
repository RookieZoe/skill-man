import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { EvidenceRail } from "./EvidenceRail";

describe("EvidenceRail", () => {
  it("renders all four cells with provided values and labels", () => {
    render(
      <EvidenceRail
        directoryIdentity="my-skill"
        canonicalEntity="/Users/test/skills/my-skill"
        sourceRelease="v1.2.0 (commit abc1234)"
        activationEvidence="Distributed to 2 target groups"
        health="healthy"
      />,
    );

    expect(
      screen.getByRole("region", { name: "Skill evidence rail" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Directory identity")).toBeInTheDocument();
    expect(screen.getByText("my-skill")).toBeInTheDocument();
    expect(screen.getByText("Canonical entity")).toBeInTheDocument();
    expect(screen.getByText("/Users/test/skills/my-skill")).toBeInTheDocument();
    expect(screen.getByText("Source release")).toBeInTheDocument();
    expect(screen.getByText("v1.2.0 (commit abc1234)")).toBeInTheDocument();
    expect(screen.getByText("Distribution status")).toBeInTheDocument();
    expect(
      screen.getByText("Distributed to 2 target groups"),
    ).toBeInTheDocument();
  });

  it("applies warning tone for source_snapshot_mismatch", () => {
    const { container } = render(
      <EvidenceRail
        directoryIdentity="mismatched-skill"
        canonicalEntity="/Users/test/skills/mismatched-skill"
        sourceRelease="main (commit def5678)"
        activationEvidence="Distributed to 1 target group"
        health="source_snapshot_mismatch"
      />,
    );

    const rail = container.querySelector(".evidence-rail");
    expect(rail).toHaveClass("evidence-rail--warning");
    expect(rail).toHaveAttribute("data-tone", "warning");
  });

  it("applies danger tone for broken health", () => {
    const { container } = render(
      <EvidenceRail
        directoryIdentity="broken-skill"
        canonicalEntity="/Users/test/skills/broken-skill"
        sourceRelease="main"
        activationEvidence="Distributed to 1 target group"
        health="broken"
      />,
    );

    const rail = container.querySelector(".evidence-rail");
    expect(rail).toHaveClass("evidence-rail--danger");
    expect(rail).toHaveAttribute("data-tone", "danger");
  });

  it("is strictly read-only with zero interactive controls", () => {
    render(
      <EvidenceRail
        directoryIdentity="read-only-skill"
        canonicalEntity="/Users/test/skills/read-only-skill"
        sourceRelease="v2.0.0"
        activationEvidence="Not distributed"
        health="healthy"
      />,
    );

    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
});
