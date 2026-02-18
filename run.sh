/etc/init.d/ntp start
ulimit -n 65535
echo "Starting doordarshan-media-server"
export GST_DEBUG=3
./doordarshan-media-server
echo "Started doordarshan-media-server"
