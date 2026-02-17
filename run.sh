#echo "Starting cloud_sql_proxy"
#./cloud_sql_proxy -instances=doordarshan:asia-south1:doordarshan=tcp:3306 &
#echo "Started cloud_sql_proxy"
/etc/init.d/ntp start
ulimit -n 65535
echo "Starting doordarshan-media-server"
export GST_DEBUG=3
./doordarshan-media-server
echo "Started doordarshan-media-server"
