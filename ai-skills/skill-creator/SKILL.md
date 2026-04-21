---
name: skill-creator
description: Create new skills, modify and improve existing skills, and run lightweight review loops. Use when users want to create a skill from scratch, edit an existing skill, try realistic test prompts, or improve when and how a skill should trigger.
---

# Skill Creator

A skill for creating new skills and iteratively improving them.

At a high level, the process of creating a skill goes like this:

- Decide what you want the skill to do and roughly how it should do it
- Write a draft of the skill
- Create a few realistic test prompts and run them with the skill
- Help the user review the results qualitatively, with simple objective checks
  when the task has clear pass/fail requirements
- Rewrite the skill based on feedback from those examples
- Repeat until you're satisfied
- Expand the test set only when the skill is important enough to justify it

Your job when using this skill is to figure out where the user is in this process and then jump in and help them progress through these stages. So for instance, maybe they're like "I want to make a skill for X". You can help narrow down what they mean, write a draft, write the test prompts, decide how they want to review the outputs, run the prompts, and repeat.

On the other hand, maybe they already have a draft of the skill. In this case you can go straight to trying it on a few realistic prompts and improving from there.

Of course, you should always be flexible and if the user is like "I don't need to run a bunch of tests, just vibe with me", you can do that instead.

Then after the skill is done, you can help tune the skill description so it
triggers in the right situations. Keep this lightweight unless the user
explicitly asks for a deeper review loop.

Cool? Cool.

## Communicating with the user

The skill creator is liable to be used by people across a wide range of familiarity with coding jargon. Some users may be comfortable editing JSON and running scripts; others may just want help turning a repeated workflow into clear instructions. Pay attention to context cues and adjust your language.

So please pay attention to context cues to understand how to phrase your communication! In the default case, just to give you some idea:

- "evaluation" is borderline, but OK; "test prompt" or "review" is often clearer
- for "JSON" and "assertion" you want to see serious cues from the user that they know what those things are before using them without explaining them

It's OK to briefly explain terms if you're in doubt, and feel free to clarify terms with a short definition if you're unsure if the user will get it.

---

## Creating a skill

### Capture Intent

Start by understanding the user's intent. The current conversation might already contain a workflow the user wants to capture (e.g., they say "turn this into a skill"). If so, extract answers from the conversation history first — the tools used, the sequence of steps, corrections the user made, input/output formats observed. The user may need to fill the gaps, and should confirm before proceeding to the next step.

1. What should this skill enable the agent to do?
2. When should this skill trigger? (what user phrases/contexts)
3. What's the expected output format?
4. Should we set up test cases to verify the skill works? Skills with objectively verifiable outputs (file transforms, data extraction, code generation, fixed workflow steps) benefit from test cases. Skills with subjective outputs (writing style, art) often don't need them. Suggest the appropriate default based on the skill type, but let the user decide.

### Interview and Research

Proactively ask questions about edge cases, input/output formats, example files, success criteria, and dependencies. Wait to write test prompts until you've got this part ironed out.

Check available MCPs - if useful for research (searching docs, finding similar skills, looking up best practices), research in parallel via subagents if available, otherwise inline. Come prepared with context to reduce burden on the user.

### Write the SKILL.md

Based on the user interview, fill in these components:

- **name**: Skill identifier
- **description**: When to trigger, what it does. This is the primary triggering mechanism - include both what the skill does AND specific contexts for when to use it. All "when to use" info goes here, not in the body. Agents often underuse skills when the description is too narrow, so make descriptions specific and a little proactive. For instance, instead of "How to build a simple fast dashboard to display internal data.", you might write "Build simple fast dashboards for internal metrics, data visualization, CSV summaries, and company reporting, even when the user does not explicitly say 'dashboard.'"
- **compatibility**: Required tools, dependencies (optional, rarely needed)
- **the rest of the skill :)**

### Skill Writing Guide

#### Anatomy of a Skill

```
skill-name/
├── SKILL.md (required)
│   ├── YAML frontmatter (name, description required)
│   └── Markdown instructions
└── Bundled Resources (optional)
    ├── scripts/    - Executable code for deterministic/repetitive tasks
    ├── references/ - Docs loaded into context as needed
    └── assets/     - Files used in output (templates, icons, fonts)
```

#### Progressive Disclosure

Skills use a three-level loading system:
1. **Metadata** (name + description) - Always in context (~100 words)
2. **SKILL.md body** - In context whenever skill triggers (<500 lines ideal)
3. **Bundled resources** - As needed (unlimited, scripts can execute without loading)

These word counts are approximate and you can feel free to go longer if needed.

**Key patterns:**
- Keep SKILL.md under 500 lines; if you're approaching this limit, add an additional layer of hierarchy along with clear pointers about where the model using the skill should go next to follow up.
- Reference files clearly from SKILL.md with guidance on when to read them
- For large reference files (>300 lines), include a table of contents

**Domain organization**: When a skill supports multiple domains/frameworks, organize by variant:
```
cloud-deploy/
├── SKILL.md (workflow + selection)
└── references/
    ├── aws.md
    ├── gcp.md
    └── azure.md
```
The agent reads only the relevant reference file.

#### Principle of Lack of Surprise

This goes without saying, but skills must not contain malware, exploit code, or any content that could compromise system security. A skill's contents should not surprise the user in their intent if described. Don't go along with requests to create misleading skills or skills designed to facilitate unauthorized access, data exfiltration, or other malicious activities. Things like a "roleplay as an XYZ" are OK though.

#### Writing Patterns

Prefer using the imperative form in instructions.

**Defining output formats** - You can do it like this:
```markdown
## Report structure
ALWAYS use this exact template:
# [Title]
## Executive summary
## Key findings
## Recommendations
```

**Examples pattern** - It's useful to include examples. You can format them like this (but if "Input" and "Output" are in the examples you might want to deviate a little):
```markdown
## Commit message format
**Example 1:**
Input: Added user authentication with JWT tokens
Output: feat(auth): implement JWT-based authentication
```

### Writing Style

Try to explain to the model why things are important in lieu of heavy-handed musty MUSTs. Use theory of mind and try to make the skill general and not super-narrow to specific examples. Start by writing a draft and then look at it with fresh eyes and improve it.

### Test Prompts

After writing the skill draft, come up with 2-3 realistic test prompts — the
kind of thing a real user would actually say. Share them with the user: [you
don't have to use this exact language] "Here are a few test cases I'd like to
try. Do these look right, or do you want to add more?" Then run them if the
user wants a concrete check.

For most skill edits, keep the prompts in the conversation. If the skill is
likely to be reused or iterated on, save a short `test-prompts.md` beside the
skill with the prompt, expected result, and any input files.

## Running and reviewing test prompts

Keep the review loop lightweight by default. The goal is to learn whether the
skill changes behavior in the direction the user wants, not to build a
measurement harness.

If you save results, put them in `<skill-name>-workspace/` as a sibling to the
skill directory. Organize results by iteration only if that makes the review
clearer. Don't create a large folder structure upfront.

### Step 1: Run the prompts

For each test prompt, run one task using the current skill:

```
Execute this task:
- Skill path: <path-to-skill>
- Task: <test prompt>
- Input files: <test files if any, or "none">
- Save outputs to: <workspace>/iteration-<N>/test-<ID>/outputs/
- Outputs to save: <what the user cares about — e.g., "the .docx file", "the final CSV">
```

Only run side-by-side comparisons when the user asks for them, or when the
change is risky enough that comparison would clearly help. Otherwise, keep
attention on the quality of the current skill's output.

### Step 2: Note simple checks

While runs are in progress, write down any objective checks that matter. Good
checks are statements a reviewer can verify from the output, such as "the CSV
contains a `margin_percent` column" or "the answer cites the source file." Avoid
forcing pass/fail checks onto subjective work like tone, taste, or strategy.

### Step 3: Review with the user

When outputs are ready, show them to the user in the most convenient format:

- For a few plain-text outputs, summarize them in the conversation and ask what
  should change.
- For file outputs or several test prompts, point the user to the result paths
  and summarize what changed.
- If you wrote objective checks, grade them plainly and treat the results as
  review notes, not as a scorecard.

### Step 4: Read the feedback

When the user gives feedback, focus your improvements on the test cases where
they had specific complaints. Empty or minimal feedback usually means the output
was fine enough for that example.

---

## Improving the skill

This is the heart of the loop. You've run the test cases, the user has reviewed the results, and now you need to make the skill better based on their feedback.

### How to think about improvements

1. **Generalize from the feedback.** The big picture thing that's happening here is that we're trying to create skills that can be used a million times (maybe literally, maybe even more who knows) across many different prompts. Here you and the user are iterating on only a few examples over and over again because it helps move faster. The user knows these examples in and out and it's quick for them to assess new outputs. But if the skill you and the user are codeveloping works only for those examples, it's useless. Rather than put in fiddly overfitty changes, or oppressively constrictive MUSTs, if there's some stubborn issue, you might try branching out and using different metaphors, or recommending different patterns of working. It's relatively cheap to try and maybe you'll land on something great.

2. **Keep the prompt lean.** Remove things that aren't pulling their weight. Make sure to read the transcripts, not just the final outputs — if it looks like the skill is making the model waste a bunch of time doing things that are unproductive, you can try getting rid of the parts of the skill that are making it do that and seeing what happens.

3. **Explain the why.** Try hard to explain the **why** behind everything you're asking the model to do. Today's LLMs are *smart*. They have good theory of mind and when given a good harness can go beyond rote instructions and really make things happen. Even if the feedback from the user is terse or frustrated, try to actually understand the task and why the user is writing what they wrote, and what they actually wrote, and then transmit this understanding into the instructions. If you find yourself writing ALWAYS or NEVER in all caps, or using super rigid structures, that's a yellow flag — if possible, reframe and explain the reasoning so that the model understands why the thing you're asking for is important. That's a more humane, powerful, and effective approach.

4. **Look for repeated work across test cases.** Read the transcripts from the test runs and notice if the subagents all independently wrote similar helper scripts or took the same multi-step approach to something. If all 3 test cases resulted in the subagent writing a `create_docx.py` or a `build_chart.py`, that's a strong signal the skill should bundle that script. Write it once, put it in `scripts/`, and tell the skill to use it. This saves every future invocation from reinventing the wheel.

Skill improvements should generalize beyond the examples in front of you. Take
time to understand what the user actually wants, write a draft revision, then
look at it again with fresh eyes before calling it done.

### The iteration loop

After improving the skill:

1. Apply your improvements to the skill
2. Rerun the useful test prompts into a new `iteration-<N+1>/` directory
3. Wait for the user to review and tell you what's still off
4. Improve again, repeat

Keep going until:
- The user says they're happy
- The feedback is all empty (everything looks good)
- You're not making meaningful progress

---

## Description Tuning

The description field in `SKILL.md` frontmatter is the primary mechanism that
helps an agent decide whether the skill is relevant. After creating or improving
a skill, offer to tune the description for better triggering accuracy.

### Step 1: Generate trigger examples

Create a set of trigger examples — a mix of should-trigger and
should-not-trigger. Plain bullets are enough:

- Should trigger: "the user prompt"
- Should not trigger: "a nearby prompt that should use a different skill"

The queries must be realistic and something a user would actually type. Not
abstract requests, but requests that are concrete and specific and have a good
amount of detail. For instance, file paths, personal context about the user's job
or situation, column names and values, company names, URLs. A little bit of
backstory. Some might be in lowercase or contain abbreviations or typos or casual
speech. Use a mix of different lengths, and focus on edge cases rather than
making them clear-cut.

Bad: `"Format this data"`, `"Extract text from PDF"`, `"Create a chart"`

Good: `"ok so my boss just sent me this xlsx file (its in my downloads, called something like 'Q4 sales final FINAL v2.xlsx') and she wants me to add a column that shows the profit margin as a percentage. The revenue is in column C and costs are in column D i think"`

For the **should-trigger** queries (8-10), think about coverage. You want different phrasings of the same intent — some formal, some casual. Include cases where the user doesn't explicitly name the skill or file type but clearly needs it. Throw in some uncommon use cases and cases where this skill competes with another but should win.

For the **should-not-trigger** queries (8-10), the most valuable ones are the near-misses — queries that share keywords or concepts with the skill but actually need something different. Think adjacent domains, ambiguous phrasing where a naive keyword match would trigger but shouldn't, and cases where the query touches on something the skill does but in a context where another tool is more appropriate.

The key thing to avoid: don't make should-not-trigger queries obviously irrelevant. "Write a fibonacci function" as a negative test for a PDF skill is too easy — it doesn't test anything. The negative cases should be genuinely tricky.

### Step 2: Review with the user

Show the trigger examples to the user and ask whether the boundaries look right.
This step matters because bad trigger examples lead to bad descriptions.

### Step 3: Revise the description

Use the reviewed examples to revise the description by hand. Aim for:

- Clear should-trigger contexts
- A few near-miss phrases the description should avoid overclaiming
- Specific nouns and workflows users actually mention
- No dependency on product-specific terminology unless the skill is truly tied
  to that product

### How skill triggering works

Understanding the triggering mechanism helps design better examples. Skills are
usually advertised to the agent through their name and description. The important
thing to know is that agents may skip skills for simple one-step tasks they can
handle directly. Complex, multi-step, or specialized queries are more likely to
trigger a skill when the description matches.

This means your trigger examples should be substantive enough that the agent
would actually benefit from consulting a skill. Simple queries like "read file X"
are poor examples.

### Step 4: Apply the result

Update the skill's `SKILL.md` frontmatter. Show the user the before/after and
explain why the new version should trigger better.

---

### Validate

If you need a quick structural check, run `scripts/quick_validate.py` against the
skill directory. This only validates `SKILL.md` frontmatter; it does not judge
whether the skill is useful.

---

## Environment Notes

The core workflow is the same across environments: draft → test prompt → review
with the user → improve → repeat. Adapt the mechanics to whatever the current
environment can do.

**Running test cases**: If subagents or background agents are available, use them
for independent test runs. If not, run the prompts yourself one at a time. The
human review step matters more than strict isolation.

**Reviewing results**: Present results directly in the conversation. For each
test case, show the prompt and the output summary. If the output is a file the
user needs to inspect, save it to the filesystem and point to the path.

**The iteration loop**: Same as before — improve the skill, rerun the useful test
prompts, ask for feedback. You can still organize results into iteration
directories on the filesystem if helpful.

**Updating an existing skill**: The user might be asking you to update an existing skill, not create a new one. In this case:
- **Preserve the original name.** Note the skill's directory name and `name` frontmatter field -- use them unchanged. E.g., if the installed skill is `research-helper`, output `research-helper.skill` (not `research-helper-v2`).
- **Copy to a writeable location before editing.** The installed skill path may be read-only. Copy to `/tmp/skill-name/`, edit there, and package from the copy.
- **If creating a distributable file manually, stage in `/tmp/` first**, then
  copy to the output directory -- direct writes may fail due to permissions.

---

Repeating one more time the core loop here for emphasis:

- Figure out what the skill is about
- Draft or edit the skill
- Run realistic test prompts with the skill
- Review the outputs with the user
- Repeat until you and the user are satisfied
- Leave the final skill in place for the user to review.

Please add steps to your TodoList, if you have such a thing, to make sure you
don't forget.

Good luck!
