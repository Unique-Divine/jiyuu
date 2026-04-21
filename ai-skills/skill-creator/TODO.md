# skill-creator — follow-ups

Open items while this skill settles after the slim-down.

## Tooling / scripts

- [ ] Document when and how to run `scripts/quick_validate.py`, including exact
      command (`python …/quick_validate.py <skill_dir>`).
- [ ] Resolve the PyYAML dependency smell (either document `pip install pyyaml`,
      switch to slice + `yq`, or rewrite validation).
- [ ] Decide whether `scripts/utils.py` stays for skill authors who write their
      own Python helpers — if yes, add a sentence in `SKILL.md`; if no, drop it.

## Skill content

- [ ] Optionally add source attribution in `LICENSE.md` appendix if you share
      this fork publicly (derivative of Apache-licensed upstream).

## Derived skills

- [x] Extract Markdown task conventions into `md-tasks` skill (`md-tasks/
      SKILL.md`).
