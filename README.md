# volume-limiting-controller

This project is a workaround for a missing feature in kubernetes on AWS. If you have been having problems with EBS volumes stuck in the "attaching" state forever, or had nodes where pods never initialize properly
because they are out of network interfaces (even though they aren't), this may be of help to you!

A typical modern AWS instance uses Amazon Nitro for all external I/O. There is a [limit of 28 (or so)
Nitro "attachments" per instance](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/volume_limits.html), no matter the instance size. But Kubernetes
does not take this limit into account directly. Kubernetes assumes that it can mount up to 25 EBS volumes per host for these instances.

This hardcoded limit is too high, because a Nito attachment includes all network interfaces and disks, including local SSDs. So for example,
if like Xometry you are running c5.4xlarge nodes with local SSDs for the docker layer storage, your "base attachments" for the node should be

| type              | attachments |
|-------------------|------------:|
| root EBS volume   |           1 |
| SSD volume        |           1 |
| [ENIs](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/using-eni.html#AvailableIpPerENI) | 8 |
| **Total**         |      **10** |

This leaves only 18 attachments available for EBS volumes, not 25!

Since the 25 limit is currently hard-coded into the kubernetes controller code and is not easily configurable (and not configurable at all
if you're using EKS), we went a different route and have written a simple controller. When a node is at its computed attachment limit, this
controller will taint the node to make sure that new pods are not scheduled onto it. If/when volumes are removed from the node, the taint
is removed and scheduling can resume.

## Limitations

* This controller currently runs as a cron job every 5 minutes, so if many pods with volumes are scheduled all at once, it may fail to taint
  the node quickly-enough to prevent problems. A future improvement would be to somehow watch nodes being scheduled and do tainting more immediately.
* The node taint that is added prevents *all* pods from being scheduled onto this node, when really we only need to prevent pods with EBS volumes
  from being scheduled. A future improvement could be to automatically (via a mutating admission controller) add a toleration to pods which don't
  have persistent volumes, so that this taint is more focused.
* This controller assumes that all persistent volumes are EBS-backed and does not take NFS or other volume types into account. This controller
  also assumes that kubernetes persistent volumes are the only way volumes are mounted, and not direct EBS volumes.

## Installation

Build the container using `docker build` and push it to a location of your choosing. Prebuilt docker containers are currently unavailable.

Install the helm chart using `helm install volume-limiting-controller charts/volume-limiting-controller --set ...`. The following valuesm
may be customized for your chart:

* image.repository and image.tag - *required* 
* runFrequency - how often the controller should run, in minutes - default is 5 minutes
* serviceAccount.create and serviceAccount.name - by default, this chart will create a cluster role, cluster role binding, and service account
  to grant the controller the necessary kubernetes permissions to list pods and nodes, and taint and untaint nodes. If for security reasons
  you need to do this in some other way, you can create your own service account instead of having the chart make one for you.
