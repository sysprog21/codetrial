# Provider pooling

Provider pooling spreads rooms over more than one LiveKit project, so
concurrent interviews draw on several projects' quotas instead of exhausting
one. A single-project deployment needs none of this and is unaffected by it.

## Adding a project

Add one `config/codetrial.env.<id>` per extra project, each with its own
`LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, and optionally
`GOOGLE_API_KEY`. The credentials already in the environment or in
`config/codetrial.env.local` become the provider called `primary`.

The id is the part after `codetrial.env.`, and it must look like a GitHub
username: 1 to 39 letters, digits, or single dashes, with no leading, trailing,
or doubled dash. `primary` is reserved in any casing, so no file can shadow the
environment's credentials. `local` and `example` are the operator's own config
and the checked-in template, so neither is read as a project.

Ids are otherwise matched exactly, so on a case-insensitive filesystem
`codetrial.env.Foo` and `codetrial.env.foo` are one file, not two projects. A
file that does not parse, or that names only part of a provider, is skipped with
a warning on stderr rather than failing the load: an optional extra project must
not be able to stop the primary one from serving.

## How a room finds its project

The id travels inside the room name, `interview-<id>-xxxxxxxx`. That is the
channel the web process uses to tell a separately launched
`codetrial run-livekit ROOM` which project the candidate was handed a token
for. Rooms minted by a single-provider deployment carry no id segment and
belong to `primary`.

Give both processes the same config directory: `config/` by default, or the
directory holding the `--config` file. `codetrial run-livekit` refuses a room
that names a project it cannot see, rather than joining the wrong one. An agent
that cannot read the config directory has a pool of one and an unresolvable id,
which is exactly the case that has to fail loudly.

## Choosing which project leads

`CODETRIAL_PROVIDER_ORDER` puts the projects it names at the front of the
rotation, in the order given, so an account with quota to burn is spent before
one that costs money. Anything it does not name keeps the order it already had
and follows: a pool is capacity, so an unnamed project stays in the rotation
rather than dropping out of it. A name no project answers to, or a name listed
twice, is reported on stderr and ignored.

Rooms that carry no provider segment resolve by id, so wherever a provider
called `primary` exists, leading the rotation with another project does not
re-point them. The exception is a pool built entirely from
`config/codetrial.env.<id>` files, which has no `primary` to resolve and falls
back to whichever project is first; there, and only there, this setting does
move which project answers for a segment-less room.

## Upgrade hazard: the first dashed id

Adding the first id that contains a dash is the one change worth draining old
processes for. A binary from before dashed ids were allowed splits the room name
at the first dash rather than the last, so it reads
`interview-eu-west-xxxxxxxx` as project `eu`. It then refuses the room, which is
the safe answer, unless a project really is named `eu`, in which case the
candidate and the agent land in different projects. Room names minted before the
change are unaffected: the random suffix holds no dash, so there was only ever
one dash to split on.
