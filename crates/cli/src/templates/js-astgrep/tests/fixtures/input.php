<?php
// Deprecated MySQL functions that should be converted to PDO
class DatabaseManager {
    private $connection;
    
    public function connect() {
        // Old MySQL connection - should be converted
        $this->connection = mysql_connect('localhost', 'user', 'pass');
        mysql_select_db('my_database');
    }
    
    public function getUsers() {
        // Old MySQL query - should be converted
        $result = mysql_query("SELECT * FROM users WHERE active = 1");
        $users = array();
        
        // Old fetch method - should be converted
        while ($row = mysql_fetch_array($result)) {
            $users[] = $row;
        }
        
        return $users;
    }
    
    public function getUserById($id) {
        // Another query that needs conversion
        $query = "SELECT * FROM users WHERE id = " . $id;
        $result = mysql_query($query);
        return mysql_fetch_assoc($result);
    }
    
    public function closeConnection() {
        mysql_close($this->connection);
    }
}
?>
