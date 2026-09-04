# The RustFS IAM model

This explains how RustFS identities actually work, why the operator's
`User` and `AccessKey` CRDs are shaped the way they are, and why an
`AccessKey` needs the owning user's password. It is written against
RustFS 1.0.0-beta.8.

## Users and access keys are two different things

A **user** is an identity: a username and a password. It is what policies
attach to. The credential pair *is* the identity — the username doubles as
an access key and the password as its secret key, which is why
`rc admin user info` prints `Access Key: <username>`. You can sign S3
requests with them directly.

An **access key** (RustFS calls it a *service account*) is an additional
AK/SK pair derived from a user. One user can own many of them.

```
User "spark"  (username + password, policies attached here)
 ├── AccessKey  AKIA...1   → Secret spark-etl-credentials
 └── AccessKey  AKIA...2   → Secret spark-adhoc-credentials
```

The operator mirrors this exactly: `User` manages the identity and its
policy attachments, `AccessKey` manages one credential pair belonging to a
user and writes it into a Kubernetes Secret.

## Every access key has a parent user

There is no such thing as a standalone AK/SK in RustFS. Every access key
is parented to some user, and that parent determines what the key is
allowed to do:

- **No policy at creation** (the operator's default): the key inherits the
  parent user's policies, evaluated at request time. Changing the user's
  policies immediately changes what every one of its keys can do. The
  server reports these keys with `impliedPolicy: true`.
- **Policy at creation** (`spec.policy` on `AccessKey`): the policy is
  embedded in the key, and a request must be allowed by **both** the
  parent's policies and the embedded one. This can only narrow access, never
  widen it.

So to grant an application access you attach policies to the `User`, not to
the `AccessKey`.

### Who becomes the parent

The server's `add-service-account` API decides the parent like this:

| Caller | `targetUser` | Resulting parent |
|--------|--------------|------------------|
| Any user | omitted | the caller itself |
| A service account | omitted | the caller's **parent user**, not the calling key |
| Root / owner | any user | the named user |
| Non-owner | itself or its own parent | that user (allowed) |
| Non-owner | any other user | rejected — `service account parent is outside requester scope` |

The guard is literally `owner || target_user == req_user || target_user ==
req_parent_user`, so a non-owner is confined to its own scope.

Two consequences are easy to trip over:

**Access keys do not nest.** A key created while authenticated as another
key becomes a *sibling* of that key, not a child of it. The hierarchy is
always exactly two levels deep: user → access keys.

**Only the root credential may parent a key to someone else.** The
`admin:CreateServiceAccount` permission controls *whether* a caller can
create keys, not *for whom*. This is deliberate — otherwise any holder of
that action could mint a root-parented key and escalate to full ownership.

### An access key may not reuse the username

Because the username is already an access key id, naming a service account
after its owner collides. The server resolves the id to the user, fails to
load it as a service account, and returns a 500 instead of a clean
not-found:

```console
$ rc admin service-account info $USER_ALIAS spark     # spark is also the username
✗ HTTP 500: <Code>InternalError</Code><Message>get service account failed</Message>

$ rc admin service-account info $USER_ALIAS spark-ak
✗ Service account 'spark-ak' not found                # clean miss
```

Nothing converges this by retrying, so the operator rejects
`accessKey == user` up front as a spec error, and the `rustfs-resources`
chart fails at render time. Pick any other name, or omit `accessKey` and
let the operator generate one.

## Do you need an AccessKey at all?

Since a user's own credentials already work for S3, you can point an
application straight at the `User` username/password and skip `AccessKey`
entirely. That is a legitimate setup, and it avoids the password plumbing
described below. What you give up:

- **One identity, one credential.** Two applications sharing a user share a
  secret, and rotating it breaks both at once. Access keys give each
  consumer its own independently revocable pair.
- **Independent lifecycle.** Deleting an access key does not disturb the
  user; rotating the user's password does not disturb its access keys.
- **Narrower scoping.** An access key can carry an embedded policy that
  further restricts it below the user's own permissions. A user credential
  always carries the user's full permissions.
- **Expiry.** Access keys can be given an expiration; user credentials
  cannot.

Rule of thumb: one workload per identity is fine on the user credential;
several workloads, or anything needing per-consumer revocation or reduced
scope, wants access keys.

## How `AccessKey` issues keys

The operator issues keys with its **admin credential**, naming the owner via
`targetUser`. The `AccessKey` CR therefore needs nothing but the username:

```yaml
spec:
  connection: { clusterRef: default }
  user: spark          # becomes targetUser
```

**The connection must hold RustFS root.** The server's guard is
`owner || target_user == req_user || target_user == req_parent_user`, so only
an owner credential may parent a key to somebody else; a scoped admin gets
`service account parent is outside requester scope`. This is deliberate —
`admin:CreateServiceAccount` controls *whether* a caller can create keys, not
*for whom*, and without the guard any holder of that action could mint a
root-parented key and escalate (GHSA-5354).

Before 0.7.0 the operator authenticated *as the owning user*, which meant every
`AccessKey` carried `passwordRef`/`passwordFromUser` and every owning user
needed `admin:CreateServiceAccount`, `admin:ListServiceAccounts` and
`admin:RemoveServiceAccount` over itself. That was a client-library gap
([rustfs/cli#340](https://github.com/rustfs/cli/issues/340)), not a server one;
it was fixed by [#352](https://github.com/rustfs/cli/pull/352) and shipped in
rc-core 0.1.32, which the operator now requires.

Migrating from 0.6.x: delete `passwordRef` / `passwordFromUser` from every
`AccessKey`, and drop the three `admin:*ServiceAccount` grants from the owning
user's policy unless something else needs them. A leftover `passwordRef` is
rejected as a spec error rather than ignored, and the chart fails at render
time, so a stale manifest cannot silently keep working.

## Reading the server directly

If operator behaviour ever looks wrong, check the server's view. Note that
`service-account list` defaults to the *calling* identity, so without
`--user` you are looking at the admin's own keys rather than the user's —
a common source of "my keys are missing":

```sh
rc admin user info <alias> spark              # identity, status, policies
rc admin service-account list <alias> --user spark   # that user's keys
rc admin service-account list <alias>                # the alias's OWN keys
```

The `parent:` shown in the listing is the owning user. Since rc 0.1.32,
`create` also takes `--user` to parent a new key to someone else (owner
credentials only); without it the key is parented to the alias's own identity:

```sh
rc admin service-account create <alias> <AK> <SK> --user spark   # rc >= 0.1.32
```

## Other server behaviours worth knowing

- **Passwords *can* be changed in place**, contrary to what earlier versions
  of these docs said. Re-issuing `admin user add` for an existing user
  replaces the password: the old one immediately returns
  `SignatureDoesNotMatch`, while attached policies and existing access keys
  survive untouched. The operator nonetheless applies `spec.password` only
  at creation — a deliberate choice, not a server limitation — so editing
  the password Secret does not currently rotate anything.
- **Policy attachment is replace-all.** RustFS's `set-user-or-group-policy`
  endpoint has no attach/detach split, so `User.spec.policies` is fully
  declarative: whatever you list is exactly what ends up attached.
- **Attached policies cannot be deleted.** Deleting a `Policy` still
  referenced by a user fails; the CR reports the error and retries until
  nothing references it.
- **Access key length limits.** AK is at most 20 characters, SK at most 40.
  Exceeding either returns `InvalidAccessKeyLength`.
