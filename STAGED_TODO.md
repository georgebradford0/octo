# STAGED TODO

- [ ] Front the push-notification relay (`octorelay.directto.link`) with Cloudflare to absorb DDoS / bad-signature floods before they reach the t4g.nano's CPU credits. Today the box is bare on a public Elastic IP; if `/notify` ever attracts spammers, Ed25519 verifies are cheap but can still chew through burst credits at high enough volume. Cloudflare in front gives free L7 caching, rate-limits, bot challenges, and a single knob to drop traffic without SSH'ing the instance.

