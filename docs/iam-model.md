# The RustFS IAM model

This explains how RustFS identities actually work, why the operator's
`User` and `AccessKey` CRDs are shaped the way they are, and why an
`AccessKey` needs the owning user's password. It is written against
RustFS 1.0.0-beta.8.

## Users and access keys are two different things

A **user** is an identity: a username and a password. It is what policies
attach to. A user's password is not an S3 credential — you cannot sign S3
requests with it.

An **access key** (RustFS calls it a *service account*) is an AK/SK pair
that S3 clients actually authenticate with. One user can own many of them.

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

## Why `AccessKey` needs the user's password

The operator authenticates **as the user** to issue that user's keys, which
is why `AccessKey` requires `passwordRef` (or `passwordFromUser`) and why
the owning user's policies must allow `admin:CreateServiceAccount`,
`admin:ListServiceAccounts` and `admin:RemoveServiceAccount` over itself.

This is a limitation of the client library, not of RustFS. The server
accepts `targetUser`, so the admin credential alone would be enough — but
`targetUser` is not exposed by `rc-core`/`rc-s3`, so the operator cannot
send it. Tracked upstream in
[rustfs/cli#340](https://github.com/rustfs/cli/issues/340); once it lands,
the password requirement and the per-user admin actions can both be dropped.

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

The `parent:` shown in the listing is the owning user. A key created with
`rc admin service-account create` is always parented to the alias's
identity, since the CLI cannot send `targetUser` either.

## Other server behaviours worth knowing

- **Passwords cannot be changed in place.** `spec.password` is applied only
  when the user is created. Rotate credentials by replacing `AccessKey`
  resources instead.
- **Policy attachment is replace-all.** RustFS's `set-user-or-group-policy`
  endpoint has no attach/detach split, so `User.spec.policies` is fully
  declarative: whatever you list is exactly what ends up attached.
- **Attached policies cannot be deleted.** Deleting a `Policy` still
  referenced by a user fails; the CR reports the error and retries until
  nothing references it.
- **Access key length limits.** AK is at most 20 characters, SK at most 40.
  Exceeding either returns `InvalidAccessKeyLength`.
