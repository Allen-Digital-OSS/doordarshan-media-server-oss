---

## 🎯 Why TENANT Is Introduced

The `TENANT` variable enables logical and infrastructure-level isolation between different customers or business units.

This ensures:

- Independent media-server allocation per tenant
- Resource isolation (CPU, memory, bandwidth)
- No cross-tenant performance impact
- Tenant-level autoscaling
- Tenant-level billing
- Tenant-scoped monitoring and metrics

---

## ⚙ How It Works

Each tenant can be assigned:

- Dedicated media-server instances
- Separate autoscaling policies
- Independent recording pipelines
- Isolated storage paths
- Custom configuration overrides
- Tenant-scoped observability

Example:

| Tenant | Media Server Pool | Autoscaling | Billing | Isolation |
|--------|-------------------|------------|---------|-----------|
| MvN    | prod-mvn          | Enabled    | Per-tenant | Fully Isolated |
| ABC    | prod-abc          | Enabled    | Per-tenant | Fully Isolated |

---

## 🚀 Performance Protection

Without tenancy isolation:

- A large broadcast from one tenant could saturate CPU
- High recording load could impact unrelated meetings
- Bandwidth spikes could degrade global stream quality

With `TENANT`-based allocation:

- Traffic remains confined to tenant-specific infrastructure
- Scaling decisions are tenant-specific
- Failures remain tenant-scoped
- Resource exhaustion in one tenant does not impact others

---

## 📈 Tenant-Level Autoscaling

Autoscaling policies can be applied independently per tenant based on:

- Active WebRTC producers/consumers
- Remaining capacity
- CPU / memory utilization
- Recording throughput
- Custom capacity metrics

This ensures that scaling is demand-driven and tenant-specific.

---

## 💰 Tenant-Level Billing

Multi-tenancy enables granular billing models:

- Usage-based billing per tenant
- Recording duration tracking
- Bandwidth consumption metrics
- Media processing costs
- Storage usage for recordings

Each tenant’s usage can be measured independently, enabling accurate chargeback or invoicing.

---

## 📊 Tenant-Scoped Observability

Metrics can be collected and analyzed per tenant:

- Active sessions
- Autoscaling events
- Error rates
- Infrastructure utilization

This improves SLA enforcement and debugging efficiency.

---
