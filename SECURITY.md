# Security policy

## Supported versions

Only the latest release gets security fixes. Update to the latest release before you report a problem.

## Report a vulnerability

Do not open a public issue for a security problem.

1. Go to the [Security tab](https://github.com/watzon/sayso/security) of the repository.
2. Select **Report a vulnerability**. GitHub sends the report privately to the maintainer.
3. Include these items:
   - the Sayso version (Settings › General › About) and the macOS version;
   - the steps to reproduce the problem;
   - what an attacker can do with it.

Expect a first reply in one week. Please give the maintainer time to release a fix before you publish the details. The release notes will credit you, unless you ask to stay anonymous.

## What is in scope

Sayso listens to the microphone, has the Accessibility permission, and writes into other apps. These parts matter most:

- text insertion and the clipboard restore;
- API keys in the macOS Keychain;
- audio and text that Sayso sends to a speech provider or an AI provider;
- the engine sidecar (`SaysoEngine`) and its protocol;
- model downloads;
- the release workflow and the signature of the official build.

A problem in a provider's service or in a model is out of scope. Report it to that provider.

## Check an official build

The official build is signed with the Developer ID of Watzon Ventures LLC (team `MB5789APU7`) and notarized by Apple. To check an installed app:

```sh
codesign --verify --deep --strict /Applications/Sayso.app
spctl --assess --type execute -vv /Applications/Sayso.app
```

The second command must show `source=Notarized Developer ID` and the team `MB5789APU7`. A build with another signature is not from this repository's releases.
