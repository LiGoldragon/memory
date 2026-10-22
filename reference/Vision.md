# criome — Vision

A typed record graph that an agent can read, edit, query, and
share — at the level of meaning, not the level of files and
strings.

This document is aspirational. When something here becomes
reality it moves into the operational specs.

---

## What a record is

A record is a small typed thing: a node in a graph, an edge, a
chord, a colour, a build step, a sentence, a person. Each kind
has a fixed shape; what a record is at rest is exactly what it
means. Two clients writing the same record produce the same
bytes — there is no extra noise to negotiate.

Strings sit at the edges of the system, not inside it. Inside,
records refer to each other by identity. A label, a name, a
human-language rendering — all of these belong to a translation
table. The same record reads as English, Spanish, or Japanese
depending on which table the reader picks. A program, a text,
and a score are the same kind of thing once you stop confusing
their bytes with their words.

## What the criome does

The criome is to agents what an operating system is to
processes: a place where worlds live. Each agent owns its world.
The criome holds it, validates every change, and serves queries
against it.

- **Hosting.** Records persist. Their history persists. Forks
  are cheap because two histories that share a past share its
  bytes; only what diverges is new.
- **Editing at the level of meaning.** Change a chord, retract
  a fact, reshape an architecture — the unit of edit is the
  record, not the file.
- **Validation before commit.** A record only lands once it
  fits its kind's rules — its references resolve, its
  invariants hold, the principal asking for the change is
  allowed to ask.
- **Queries that are values.** A query is a record too. It
  composes, it persists, it can be subscribed to.

## Text and binary, both ways

There are two surfaces. The binary surface is what records ARE;
the text surface is how a human reads and writes them. Every
text construct corresponds to exactly one binary form, and the
reverse holds. This is not a serialiser bolted on after the
fact — it is the same definition read from either side.

The reader and writer always know the shape of what they are
exchanging. Nothing in the format describes itself. This is the
discipline that lets the bytes mean exactly one thing.

## First steps

The first records the criome handles describe **flow-graphs** —
nodes, edges, and the graphs that hold them. This is small
enough to land end-to-end and useful enough to be the way the
project starts designing the rest of itself: the architecture
diagrams that today live as pictures in markdown become records
in the criome the moment the daemon starts.

From there, kinds accrete. Each new kind shrinks the surface
where strings stand in for things the system does not yet
understand.

## What the ontology grows toward

Each new record kind absorbs a class of "free-form string for a
thing we have not specified yet." Today that is architecture.
Soon it is build plans, source code shape, relations between
records. Later it is scheduling, geography, music intervals,
colour. Far later it is audio, video, spatial composition, and
the structure of natural language itself.

This is the long line of the project: every string in the
system today is a placeholder. As the ontology grows the
placeholders collapse into typed records, and the human-language
rendering moves to translation tables.

---

## Companion docs

For implementation detail see the per-repo `ARCHITECTURE.md`
files (criome, signal, nexus, memory, forge) and the open design
questions in `workspace/reports/`.
