---
description: Core development standards for communication, code style, APIs, concurrency, and testing
alwaysApply: true
---

# Cursor Rules for development

## COMMUNICATION
- Ask questions one at a time until you're 95% confident you  understand the task.
- Terse and clear; keywords over filler; no pleasantries. User is expert—no long explanations; they ask when needed.
- Pasted errors: fix only, don't explain. Plans: short, no duplicated instructions.
- When user gives a change plus why: implement; don't echo the rationale back.
- Don't guess requirements or implementation. If unclear, ask. No unrequested "helpful" features—keep scope minimal.
- Don't affirm the user unless verified. If they're wrong, say so—don't implement incorrect requests.
- Don't run terminal commands for things you've got access to as tools, such as finding files.

## IMPLEMENTATION
- No bare literals (150, 2, 20, …). Named constants at file top—descriptive names, brief comments; easy to change.
- Code reuse whenever possible. DRY principle. Extract shared helpers; composition over copy-paste. No duplicated logic across files. But do not add helpers for only one use.
- Simplest correct solution; standard language idioms. No "just in case" code or `ensureXYZ`-style guards—run what's needed when it's needed, nothing extra.
- Stick to dependencies you already have when possible.
- Always check the docus before assuming things.
- Verify that your knowledge about libraries, frameworks and API:s is up to date.
- Encapsulation: no external use of `_` internals or private attributes. Inter-class access via public methods/properties only; add a public API if needed (avoids races/deadlocks).

## CODE STYLE
- Match existing style. Comments only for non-obvious behavior—never change logs or obvious code. No comment-before-blank-line padding.
- No changelog style comments. Comments always reflect current state and not what has been changed.
- Limit nesting (inverse logic OK). Imports at top only. Empty lines only between functions/classes; no artificial spacing or unneeded numeric type suffixes/casts.
- Compact: every line earns its place.

## ERROR HANDLING
- Narrow `try` blocks—only code that may fail at runtime acceptably. Multiple `try`s to distinguish errors. Don't wrap whole functions; don't catch logic bugs.

## BUG FIXING
- Fix root causes, not symptoms.
- Fix where bad data is created, not where it's consumed. No downstream checks/transforms for upstream mistakes (e.g. wrong block types → fix block insertion, not block processing).
- Never `time.sleep()`, delays, or timeouts to paper over races. Use `threading.Event`, `Condition`, `Semaphore`, or locks; `Event.wait()` / `Condition.wait()` for waiting—not sleep. No exceptions.

## TESTS
- Mocks only when unavoidable. Don't change production code to satisfy tests unless the code is wrong.
