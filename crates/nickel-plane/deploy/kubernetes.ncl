# Render with nickel-k8s. Build and supply your own pinned controller image.
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
{
 target=profiles.kubernetes {},
 resources=[{
  apiVersion="apps/v1",kind="Deployment",metadata={name="nickel-plane",namespace="control"},
  spec={replicas=1,strategy.type="Recreate",selector.matchLabels.app="nickel-plane",template={
   metadata.labels.app="nickel-plane",spec={
    serviceAccountName="nickel-plane",automountServiceAccountToken=false,
    containers=[{name="controller",image="registry.example.com/nickel-plane@sha256:0000000000000000000000000000000000000000000000000000000000000000",
     args=["serve","--config","/config/instance/main.ncl","--resources","/opt/nickel-plane","--targets","/config/targets.json","--k8s-resources","/opt/nickel-k8s","--store","/state"],
     securityContext={allowPrivilegeEscalation=false,capabilities.drop=["ALL"]},
     volumeMounts=[{name="state",mountPath="/state"},{name="config",mountPath="/config",readOnly=true},{name="credentials",mountPath="/credentials",readOnly=true}],
    }],
    volumes=[{name="state",persistentVolumeClaim.claimName="nickel-plane"},{name="config",configMap.name="nickel-plane-config"},{name="credentials",secret={secretName="nickel-plane-credentials",defaultMode=256}}],
   },
  }},
 }],
}
