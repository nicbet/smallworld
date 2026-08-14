To meet best textbook practices, each chapter should bridge theory with concrete architectural decisions. It should move from the "why" (the problem domain) to the "how" (the specific engine implementation), while providing clear learning objectives and a summarizing review.

Some of the best value for a chapter is: what approaches exist, why the chosen one is superior, common pitfalls, tips and tricks etc. Each chapter should also contain quite a bit of background writing and overall be meaty and verbose.

### Argument direction

Every chapter follows the same arc: **principle first, then concrete realization.**

1. Start with the general problem every engine must solve, independent of any specific engine or language.
2. Examine how mature engines (UE5, Unity, Godot, and others) have solved it, and what design pressures shaped their choices.
3. Distinguish which of those choices are durable principles and which are artifacts of a particular era, language, or platform.
4. Show how Smallworld resolves the same responsibility in its own stack (Rust, `wgpu`), and what those specific choices buy at the friction points.

Never lead with "Smallworld does X." Lead with why the problem exists, what the industry learned, and then show the concrete realization. The reader should finish each chapter understanding both the universal principle and one well-reasoned path through it.

### Heading discipline

A heading earns its place when it divides its parent into two or more peer
ideas. Do not create a lone section, subsection, or sub-subsection merely to
give a paragraph a label: merge that material into its parent instead. When a
single distinction deserves emphasis but not a table-of-contents entry, use a
short bold lead-in in the prose or a clearly labelled bullet. This keeps the
book's navigation proportional to the argument rather than to every local
point.

Here is a standard textbook blueprint template. Each Section can be further broken down into sub-sections `[Chapter Number].[Section Number].1`, `[Chapter Number].[Section Number].2` etc. where required to keep length manageable.

---

### The Textbook Chapter Blueprint Template

**Chapter [Number]: [Chapter Title]**

**Chapter Summary**
A brief, high-level introduction (1-2 paragraphs) setting the context for the chapter. It explains what subsystem is being discussed, why it is critical for a modern game engine, and how it connects to the concepts learned in previous chapters.

> **What You Will Learn in This Chapter**
>
> - Bullet point 1: Key theoretical concept.
> - Bullet point 2: Core architectural pattern.
> - Bullet point 3: Specific implementation detail or optimization.
> - Bullet point 4: Data management or rendering strategy.

**[Number].1 [High-Level Concept / Theory]**
Explores the problem domain independently of the engine's specific code. Discusses industry standards, historical approaches (e.g., UE5 vs. Unity paradigms), and the fundamental challenges the subsystem must solve. Identifies which industry patterns are durable principles and which are era-specific artifacts.

**[Number].2 [Core Architectural Design]**
Dives into the specific architecture chosen for the engine. Details the data structures, the engine/game responsibility split, and how data flows through this specific pipeline. Shows how Smallworld's stack choices produce clean answers to the friction points identified in the theory section.

**[Number].3 [Implementation Details & Subsystems]**
_Contains multiple subsections (e.g., X.3.1, X.3.2)._ Breaks down the specific mechanical implementations, thread ownership, memory budgets, and integration with the wider engine (like the Render Graph or ECS).

**[Number].4 [Edge Cases, Optimization, & Persistence]**
Addresses how the architecture scales, how it handles serialization/saving, and specific optimizations required to maintain performance budgets.

**Chapter Summary**
A concise wrap-up that reiterates the core architectural decisions made in the chapter and tees up the next chapter.
