let c = import "/libraries/nickel-tf/composition.ncl" in
let tf = import "/libraries/nickel-tf/expressions.ncl" in
{
 apiVersion="nickel-plane/v1",id="proxmox-demo",composition={name="vm",version="1"},
 policy={mode="observe"},
 units.vm={backend="opentofu",target="proxmox",schema="proxmox",
  configuration=c.compose {
   connection={provider.proxmox={endpoint="https://proxmox.example.com:8006/",insecure=false}},
   machine={resource.proxmox_virtual_environment_vm.demo={
    name="nickel-demo",node_name="pve",vm_id=9100,
    started=false,on_boot=false,protection=true,
    cpu=[{cores=2}],memory=[{dedicated=2048}],
    disk=[{datastore_id="local-lvm",interface="scsi0",size=8}],
    network_device=[{bridge="vmbr0",model="virtio",disconnected=false,enabled=null,firewall=true,mac_address=null,mtu=null,queues=null,rate_limit=null,trunks=null,vlan_id=null}],
   }},
   exports={output.vm_id.value=tf.ref ["proxmox_virtual_environment_vm","demo","vm_id"]},
  },outputs.vm_id.pointer="/outputs/vm_id",
 },
}
