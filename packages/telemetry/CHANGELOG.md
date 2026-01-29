# @codemod-com/telemetry

## 1.1.1

### Patch Changes

- e927d1b: Fix posthog-node to immediately flush after each event and make sure dispose() is called before cli exits.
  For more info regarding the options changed in this release, please refer to: https://posthog.com/docs/libraries/node
