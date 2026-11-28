- When owlyd gave FloeDrop no link, FloeDrop could fail to start with
  "FloeDrop cannot start on ...: cannot listen on ...: Cannot assign requested
  address" if the AWDL link's IPv6 address was still new. FloeDrop now waits
  up to 3 seconds for the address to become usable; if it does not, the error
  says "... has no usable IPv6 link-local address". (#92)
