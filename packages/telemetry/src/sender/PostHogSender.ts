import { PostHog } from "posthog-node";

import { redactFilePaths } from "../utils/redactFilePath.js";

import type {
  BaseEvent,
  TelemetrySender,
  TelemetrySenderOptions,
} from "../types.js";

export class PostHogSender<Event extends BaseEvent>
  implements TelemetrySender<Event>
{
  private __telemetryClient: PostHog;

  constructor(private readonly __options: TelemetrySenderOptions) {
    this.__telemetryClient = new PostHog(
      "phc_nGWKWP3t1fcNFqGi6UdstXjMf0fxx7SBeohHPSS6d2Y",
      {
        host: "https://app.posthog.com",
        flushAt: 1,
        flushInterval: 0,
      },
    );
  }
  public dispose(): Promise<unknown> {
    return this.__telemetryClient.shutdown();
  }

  public async sendEvent(
    event: Event,
    // allow to override distinctId and cloudRole in the sendEvent method directly
    optionsOverrides?: Partial<TelemetrySenderOptions>,
    isDangerous = false,
  ): Promise<void> {
    const { kind, ...rawProperties } = event;

    const distinctId =
      optionsOverrides?.distinctId ?? this.__options.distinctId;
    const cloudRole = optionsOverrides?.cloudRole ?? this.__options.cloudRole;

    const properties = isDangerous
      ? rawProperties
      : Object.entries(rawProperties).reduce<Record<string, string>>(
          (properties, [key, value]) => {
            properties[key] = redactFilePaths(String(value));

            return properties;
          },
          {},
        );

    this.__telemetryClient?.capture({
      distinctId,
      event: `codemod.${cloudRole}.${kind}`,
      properties: {
        cloudRole: cloudRole,
        ...properties,
      },
    });
  }

  public async sendDangerousEvent(
    event: Event,
    optionsOverrides?: Partial<TelemetrySenderOptions>,
  ): Promise<void> {
    return this.sendEvent(event, optionsOverrides, true);
  }
}
